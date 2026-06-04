//! Background proxy builds: the disk tier beneath the in-RAM [`FrameCache`].
//!
//! On import we kick off a background transcode of each source into a 1080p
//! **all-intra H.264** proxy (see [`cutlass_decode::build_proxy`]). Once a proxy
//! is on disk, the [`MediaPool`](crate::MediaPool) reads frames from it instead
//! of the long-GOP source, turning a 0.4–1.6 s cold seek into a flat ~9 ms one.
//! See `docs/proxy-cache/research.md` for the measurements and the full design.
//!
//! The build runs on a dedicated worker thread with its **own** decoder (it
//! re-opens the file), so it never shares ffmpeg state with the live reader.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use cutlass_decode::{ProxyConfig, ProxyStats};
use cutlass_models::MediaId;
use rustc_hash::FxHasher;
use tracing::{debug, warn};

/// Bump to invalidate every proxy on disk after a format/codec change.
const CACHE_VERSION: u32 = 1;

/// State of a media's on-disk proxy.
#[derive(Debug, Clone)]
pub enum ProxyStatus {
    /// No proxy requested.
    None,
    /// A background build is in flight.
    Building,
    /// Proxy is on disk at this path and registered as the fast reader.
    Ready(PathBuf),
    /// The build failed; the source reader is used permanently.
    Failed(String),
}

impl ProxyStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, ProxyStatus::Ready(_))
    }
}

/// A queued transcode job handed to the worker thread.
pub(crate) struct RenderJob {
    pub media: MediaId,
    pub source: PathBuf,
    pub output: PathBuf,
    pub config: ProxyConfig,
}

/// A completed (or failed) build reported back from the worker thread.
pub(crate) struct ProxyDone {
    pub media: MediaId,
    pub output: PathBuf,
    pub result: Result<ProxyStats, String>,
}

/// Owns the worker thread plus the job/completion channels.
///
/// Dropping the service closes the job channel, which ends the worker loop.
pub(crate) struct ProxyService {
    job_tx: Sender<RenderJob>,
    done_rx: Receiver<ProxyDone>,
}

impl ProxyService {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<RenderJob>();
        let (done_tx, done_rx) = mpsc::channel::<ProxyDone>();

        thread::Builder::new()
            .name("cutlass-proxy".into())
            .spawn(move || {
                // SW frame-threaded decode → HW encode (the fast build pipeline).
                while let Ok(job) = job_rx.recv() {
                    debug!(media = %job.media, ?job.output, "building proxy");
                    let result = cutlass_decode::build_proxy(&job.source, &job.output, job.config)
                        .map_err(|e| e.to_string());
                    if done_tx
                        .send(ProxyDone {
                            media: job.media,
                            output: job.output,
                            result,
                        })
                        .is_err()
                    {
                        break; // receiver gone; pool dropped.
                    }
                }
            })
            .expect("spawn cutlass-proxy worker");

        Self { job_tx, done_rx }
    }

    pub fn submit(&self, job: RenderJob) {
        // Send failure only happens if the worker died; nothing we can do here.
        let _ = self.job_tx.send(job);
    }

    /// Non-blocking drain of one completion, if any is ready.
    pub fn try_recv(&self) -> Option<ProxyDone> {
        self.done_rx.try_recv().ok()
    }
}

/// Where proxies live: content-addressed, outside the project, safe to delete.
pub fn proxy_cache_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Caches/cutlass/proxies");
    }
    std::env::temp_dir().join("cutlass/proxies")
}

/// Stable cache key for a source + proxy resolution. Content-addressed via path,
/// size, and mtime so edits to the file invalidate it but moves/renames within a
/// project do not re-key gratuitously.
pub fn render_hash(source: &Path, target_height: u32) -> String {
    let mut h = FxHasher::default();
    CACHE_VERSION.hash(&mut h);
    source.to_string_lossy().hash(&mut h);
    if let Ok(meta) = std::fs::metadata(source) {
        meta.len().hash(&mut h);
        if let Ok(mtime) = meta.modified() {
            if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                dur.as_secs().hash(&mut h);
            }
        }
    }
    target_height.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The proxy file path for a source under the cache dir.
pub fn proxy_path(source: &Path, target_height: u32) -> PathBuf {
    proxy_cache_dir().join(format!("{}.mp4", render_hash(source, target_height)))
}

/// Byte-bounded LRU eviction over the on-disk proxy directory.
///
/// Mirrors [`FrameCache`](crate::FrameCache)'s budget but for disk: the disk
/// footprint of intra proxies is large, so without this the cache grows without
/// bound (the lesson from Filmora's 42 GB `.Render`).
pub(crate) struct DiskBudget {
    cap_bytes: u64,
}

impl DiskBudget {
    pub fn new(cap_bytes: u64) -> Self {
        Self { cap_bytes }
    }

    /// Delete least-recently-modified proxies until the directory fits the cap.
    /// `keep` paths (currently-open proxies) are never deleted. Best-effort.
    pub fn prune(&self, dir: &Path, keep: &HashSet<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        let mut total: u64 = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            files.push((path, meta.len(), mtime));
        }
        if total <= self.cap_bytes {
            return;
        }
        // Oldest first.
        files.sort_by_key(|(_, _, mtime)| *mtime);
        for (path, len, _) in files {
            if total <= self.cap_bytes {
                break;
            }
            if keep.contains(&path) {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    total = total.saturating_sub(len);
                    debug!(?path, "evicted proxy to fit disk budget");
                }
                Err(e) => warn!(?path, error = %e, "failed to evict proxy"),
            }
        }
    }
}

/// Default disk budget for proxies: 8 GiB.
pub const DEFAULT_DISK_BUDGET_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_hash_is_stable_and_resolution_specific() {
        let p = Path::new("/tmp/does-not-exist.mp4");
        let a = render_hash(p, 1080);
        let b = render_hash(p, 1080);
        let c = render_hash(p, 540);
        assert_eq!(a, b, "same inputs hash the same");
        assert_ne!(a, c, "resolution is part of the key");
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn proxy_path_lives_under_cache_dir() {
        let p = proxy_path(Path::new("/tmp/x.mp4"), 1080);
        assert!(p.starts_with(proxy_cache_dir()));
        assert_eq!(p.extension().unwrap(), "mp4");
    }
}
