//! The media pool: the engine's source-frame provider.
//!
//! Owns one entry per registered media plus the single shared [`FrameCache`].
//! Every frame request goes through three tiers, cheapest first:
//!
//! 1. **RAM** — the byte-bounded [`FrameCache`] (no decode).
//! 2. **Disk proxy** — a 1080p all-intra reader, once a background build lands
//!    (O(1) seek, ~9 ms cold; see [`crate::proxy`]).
//! 3. **Source** — the original long-GOP file (slow cold seek; the fallback that
//!    makes editing work *instantly* on import, before any proxy exists).
//!
//! This is the seam the rest of the engine — timeline resolution, the
//! compositor, export — pulls source frames from.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutlass_decode::{DecodeOptions, DecodedFrame, HwAccel, ProxyConfig};
use cutlass_models::{Map, MediaId, MediaSource, Rational};

use crate::cache::{CacheStats, FrameCache, FrameKey};
use crate::error::EngineError;
use crate::media::{FrameReader, MediaReader};
use crate::proxy::{
    proxy_cache_dir, proxy_path, DiskBudget, ProxyMsg, ProxyService, ProxyStatus, RenderJob,
    DEFAULT_DISK_BUDGET_BYTES,
};

/// Info captured at proxy-request time, needed to open the proxy reader once the
/// background build finishes.
#[derive(Clone)]
struct ProxyMeta {
    frame_rate: Rational,
    duration: i64,
}

/// One registered media: its source reader, an optional faster proxy reader, and
/// the proxy build status.
struct MediaEntry {
    source: Box<dyn FrameReader>,
    proxy: Option<Box<dyn FrameReader>>,
    status: ProxyStatus,
    meta: Option<ProxyMeta>,
    last_access: u64,
}

/// A registry of decodable media backed by a shared decoded-frame cache and an
/// on-disk proxy tier.
pub struct MediaPool {
    entries: Map<MediaId, MediaEntry>,
    cache: FrameCache,
    /// Spawned lazily on the first proxy request (tests/synthetic readers never
    /// pay for a worker thread).
    proxies: Option<ProxyService>,
    budget: DiskBudget,
    proxy_config: ProxyConfig,
    /// Monotonic access counter for recency (disk-tier LRU).
    clock: u64,
    /// Monotonic priority counter; the most recently prioritized media gets the
    /// highest value, so a playhead bump always jumps the build queue.
    priority_clock: i64,
}

impl MediaPool {
    /// Create an empty pool with the default cache budget.
    pub fn new() -> Self {
        Self::with_cache(FrameCache::default())
    }

    /// Create an empty pool backed by a caller-configured cache.
    pub fn with_cache(cache: FrameCache) -> Self {
        Self {
            entries: Map::default(),
            cache,
            proxies: None,
            budget: DiskBudget::new(DEFAULT_DISK_BUDGET_BYTES),
            proxy_config: ProxyConfig::default(),
            clock: 0,
            priority_clock: 0,
        }
    }

    /// Open `media`'s file and register it for decoding.
    ///
    /// Returns the [`MediaId`] now served by the pool. Opening touches the
    /// filesystem and probes the stream, so it is comparatively expensive — do
    /// it at import time, not per frame. Does **not** start a proxy build; call
    /// [`request_proxy`](Self::request_proxy) for that.
    pub fn open(&mut self, media: &MediaSource) -> Result<MediaId, EngineError> {
        let reader = MediaReader::open(media)?;
        self.register(media.id, Box::new(reader));
        Ok(media.id)
    }

    /// Register a pre-built source reader under `media`. Replaces any existing
    /// entry for that id and drops its now-stale cached frames.
    pub fn register(&mut self, media: MediaId, reader: Box<dyn FrameReader>) {
        let existed = self
            .entries
            .insert(
                media,
                MediaEntry {
                    source: reader,
                    proxy: None,
                    status: ProxyStatus::None,
                    meta: None,
                    last_access: 0,
                },
            )
            .is_some();
        if existed {
            self.cache.invalidate_media(media);
        }
    }

    /// Remove `media` from the pool and drop its cached frames.
    pub fn remove(&mut self, media: MediaId) {
        if self.entries.remove(&media).is_some() {
            self.cache.invalidate_media(media);
        }
    }

    pub fn contains(&self, media: MediaId) -> bool {
        self.entries.contains_key(&media)
    }

    /// Fetch source frame `source_frame` of `media`, decoding on a cache miss.
    ///
    /// On a RAM miss, decode prefers the disk proxy (fast) when ready, falling
    /// back to the source. The returned [`Arc`] is shared with the cache, so
    /// holding it (e.g. while compositing) costs no copy and keeps the frame
    /// alive even if it is later evicted.
    pub fn frame(
        &mut self,
        media: MediaId,
        source_frame: i64,
    ) -> Result<Arc<DecodedFrame>, EngineError> {
        self.clock += 1;
        let clock = self.clock;
        let entry = self
            .entries
            .get_mut(&media)
            .ok_or(EngineError::UnknownMedia(media))?;
        entry.last_access = clock;
        let key = FrameKey::new(media, source_frame);
        // Prefer the proxy reader once it's ready; else the source reader.
        let reader: &mut dyn FrameReader = match entry.proxy.as_mut() {
            Some(proxy) => proxy.as_mut(),
            None => entry.source.as_mut(),
        };
        // Disjoint borrows: `reader` borrows `self.entries`, the cache call
        // borrows `self.cache`. Only a miss runs the (decoding) closure.
        self.cache
            .get_or_try_insert_with(key, || reader.read(source_frame))
    }

    /// Start a background proxy build for `media` (idempotent).
    ///
    /// Returns immediately. If a proxy already exists on disk (a prior run), it
    /// is adopted synchronously; otherwise a worker thread transcodes it and
    /// [`poll_proxies`](Self::poll_proxies) installs it when done.
    pub fn request_proxy(&mut self, media: &MediaSource) {
        let target_height = self.proxy_config.target_height;
        let path = proxy_path(&media.path, target_height);

        {
            let Some(entry) = self.entries.get_mut(&media.id) else {
                return;
            };
            if !matches!(entry.status, ProxyStatus::None) {
                return; // already building, ready, or permanently failed.
            }
            entry.meta = Some(ProxyMeta {
                frame_rate: media.frame_rate,
                duration: media.duration,
            });

            // Cross-run reuse: adopt an existing proxy without re-rendering.
            if path.exists() {
                match open_proxy_reader(media.id, &path, media.frame_rate, media.duration) {
                    Ok(reader) => {
                        entry.proxy = Some(reader);
                        entry.status = ProxyStatus::Ready(path);
                        return;
                    }
                    // Stale/corrupt: drop it and rebuild below.
                    Err(_) => {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            entry.status = ProxyStatus::Building { progress: 0.0 };
        }

        let _ = std::fs::create_dir_all(proxy_cache_dir());
        let config = self.proxy_config;
        let total_frames = media.duration.max(0) as u64;
        let service = self.proxies.get_or_insert_with(ProxyService::spawn);
        service.submit(RenderJob {
            media: media.id,
            source: media.path.clone(),
            output: path,
            config,
            total_frames,
            priority: 0,
        });
    }

    /// Bump `media` to the front of the build queue (e.g. it's under the
    /// playhead). No-op if its proxy is already built or not yet requested.
    pub fn prioritize_proxy(&mut self, media: MediaId) {
        if let Some(service) = self.proxies.as_ref() {
            self.priority_clock += 1;
            service.prioritize(media, self.priority_clock);
        }
    }

    /// Pause or resume background proxy builds. Pause during heavy interaction
    /// (scrub/playback) so transcodes don't steal cycles from the live path;
    /// resume when idle.
    pub fn set_background_paused(&mut self, paused: bool) {
        if let Some(service) = self.proxies.as_ref() {
            service.set_paused(paused);
        }
    }

    /// Install any finished proxy builds and keep the disk cache within budget.
    ///
    /// Cheap and non-blocking; call it on the frame path (e.g. once per
    /// [`Engine::frame_at`](crate::Engine::frame_at)).
    pub fn poll_proxies(&mut self) {
        let mut messages = Vec::new();
        if let Some(service) = self.proxies.as_ref() {
            while let Some(msg) = service.try_recv() {
                messages.push(msg);
            }
        }
        if messages.is_empty() {
            return;
        }

        let mut installed_any = false;
        for msg in messages {
            match msg {
                ProxyMsg::Progress { media, progress } => {
                    if let Some(entry) = self.entries.get_mut(&media) {
                        // Only advance a still-building status; never clobber a
                        // Ready/Failed result with a late progress message.
                        if let ProxyStatus::Building { progress: p } = &mut entry.status {
                            *p = progress;
                        }
                    }
                }
                ProxyMsg::Done(done) => {
                    let Some(entry) = self.entries.get_mut(&done.media) else {
                        continue;
                    };
                    match done.result {
                        Ok(_stats) => match entry.meta.clone() {
                            Some(meta) => match open_proxy_reader(
                                done.media,
                                &done.output,
                                meta.frame_rate,
                                meta.duration,
                            ) {
                                Ok(reader) => {
                                    entry.proxy = Some(reader);
                                    entry.status = ProxyStatus::Ready(done.output);
                                    installed_any = true;
                                }
                                Err(e) => entry.status = ProxyStatus::Failed(e.to_string()),
                            },
                            None => entry.status = ProxyStatus::Failed("missing proxy meta".into()),
                        },
                        Err(e) => entry.status = ProxyStatus::Failed(e),
                    }
                }
            }
        }

        if !installed_any {
            return;
        }

        // Never delete a proxy we currently have open.
        let keep: HashSet<PathBuf> = self
            .entries
            .values()
            .filter_map(|e| match &e.status {
                ProxyStatus::Ready(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        self.budget.prune(&proxy_cache_dir(), &keep);
    }

    /// Current proxy status for `media`, if registered.
    pub fn proxy_status(&self, media: MediaId) -> Option<&ProxyStatus> {
        self.entries.get(&media).map(|e| &e.status)
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    pub fn cache(&self) -> &FrameCache {
        &self.cache
    }
}

/// Open a proxy file as a software-decoding [`MediaReader`] under `id`.
///
/// Software decode is deliberate: the proxy is all-intra, so cold seeks are
/// single-frame, and software beats hardware on that latency (~9 ms vs ~62 ms).
fn open_proxy_reader(
    id: MediaId,
    path: &Path,
    frame_rate: Rational,
    duration: i64,
) -> Result<Box<dyn FrameReader>, EngineError> {
    // The reader only needs id/path/frame_rate/duration; dimensions come from the
    // proxy stream itself at decode time.
    let proxy_source = MediaSource {
        id,
        path: path.to_path_buf(),
        width: 0,
        height: 0,
        frame_rate,
        duration,
        has_audio: false,
    };
    let reader = MediaReader::open_with(
        &proxy_source,
        DecodeOptions::default().hw_accel(HwAccel::None),
    )?;
    Ok(Box::new(reader))
}

impl Default for MediaPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutlass_decode::{PixelFormat, Plane};

    /// A reader that fabricates frames and counts how often it actually runs,
    /// so we can prove the cache prevents re-decoding.
    struct CountingReader {
        reads: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl FrameReader for CountingReader {
        fn read(&mut self, source_frame: i64) -> Result<DecodedFrame, EngineError> {
            self.reads.set(self.reads.get() + 1);
            Ok(DecodedFrame {
                width: 2,
                height: 2,
                pts_ticks: source_frame,
                format: PixelFormat::Rgba8,
                planes: vec![Plane {
                    data: vec![0u8; 16],
                    stride: 16,
                }],
            })
        }
    }

    fn counting() -> (Box<dyn FrameReader>, std::rc::Rc<std::cell::Cell<usize>>) {
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        (
            Box::new(CountingReader {
                reads: reads.clone(),
            }),
            reads,
        )
    }

    #[test]
    fn unknown_media_errors() {
        let mut pool = MediaPool::new();
        let err = pool.frame(MediaId::from_raw(1), 0).unwrap_err();
        assert!(matches!(err, EngineError::UnknownMedia(_)));
    }

    #[test]
    fn second_request_is_served_from_cache() {
        let mut pool = MediaPool::new();
        let (reader, reads) = counting();
        let media = MediaId::from_raw(1);
        pool.register(media, reader);

        let a = pool.frame(media, 10).unwrap();
        let b = pool.frame(media, 10).unwrap();

        assert_eq!(reads.get(), 1, "decode happens once");
        assert!(Arc::ptr_eq(&a, &b), "same cached frame returned");
        assert_eq!(pool.cache_stats().hits, 1);
        assert_eq!(pool.cache_stats().misses, 1);
    }

    #[test]
    fn distinct_frames_each_decode_once() {
        let mut pool = MediaPool::new();
        let (reader, reads) = counting();
        let media = MediaId::from_raw(1);
        pool.register(media, reader);

        pool.frame(media, 0).unwrap();
        pool.frame(media, 1).unwrap();
        pool.frame(media, 0).unwrap();

        assert_eq!(reads.get(), 2, "frame 0 reused, frame 1 decoded once");
    }

    #[test]
    fn reregistering_media_drops_cached_frames() {
        let mut pool = MediaPool::new();
        let media = MediaId::from_raw(1);

        let (reader, _) = counting();
        pool.register(media, reader);
        pool.frame(media, 5).unwrap();
        assert!(pool.cache().contains(FrameKey::new(media, 5)));

        let (reader2, reads2) = counting();
        pool.register(media, reader2);
        assert!(!pool.cache().contains(FrameKey::new(media, 5)), "cache purged");

        pool.frame(media, 5).unwrap();
        assert_eq!(reads2.get(), 1, "served by the new reader, not stale cache");
    }

    #[test]
    fn remove_purges_media_from_cache() {
        let mut pool = MediaPool::new();
        let media = MediaId::from_raw(1);
        let (reader, _) = counting();
        pool.register(media, reader);
        pool.frame(media, 3).unwrap();

        pool.remove(media);

        assert!(!pool.contains(media));
        assert!(!pool.cache().contains(FrameKey::new(media, 3)));
        assert!(matches!(
            pool.frame(media, 3).unwrap_err(),
            EngineError::UnknownMedia(_)
        ));
    }
}
