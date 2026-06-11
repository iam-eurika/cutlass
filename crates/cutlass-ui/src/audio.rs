//! Audio playback: device output, timeline mixing, and the playback master
//! clock (playback roadmap Phase 3).
//!
//! Three players, all decoupled by lock-free state:
//!
//! - The **UI thread** owns transport intent: play/seek bump an *epoch* and
//!   reset the shared clock atomically, then notify the mixer. It also reads
//!   the clock (`AudioHandle::current_tick`) from the playback timer.
//! - The **mixer thread** owns `AudioReader`s and the timeline snapshot. It
//!   decodes + sums every audible span into fixed-size stereo blocks, tagged
//!   with the epoch they were mixed for, and feeds them through a bounded
//!   channel sized to ~100ms.
//! - The **device callback** pops blocks, drops any with a stale epoch
//!   (seek flush without locks), spreads stereo onto the device's channel
//!   count, and advances `frames_played` — *consumed frames are the clock*,
//!   so A/V sync is device-true regardless of buffer depth.
//!
//! The clock counts only real consumed frames: an underrun stalls the
//! playhead briefly instead of letting video run away from audio. Silence
//! (no clips under the playhead) still produces zero-blocks, so the audio
//! device paces silent timelines too; the wall-clock transport is only a
//! fallback for machines with no output device (and for shuttle speeds,
//! which play muted — see `transport.rs`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use cutlass_decoder::{AUDIO_CHANNELS, AudioReader};
use tracing::{debug, info, warn};

/// Frames per mixed block. 1024 @ 48kHz ≈ 21ms.
const BLOCK_FRAMES: usize = 1024;
/// Blocks in flight device-ward. 6 × 21ms ≈ 128ms of buffered audio — also
/// the worst-case latency for a mid-playback edit to become audible.
const BLOCK_CAPACITY: usize = 6;

// ---------------------------------------------------------------------------
// Snapshot: what the timeline sounds like (worker → mixer)
// ---------------------------------------------------------------------------

/// One audible clip in rational time. The mixer converts to device frames.
pub struct AudioSpan {
    pub path: PathBuf,
    /// Timeline placement, sequence ticks at the snapshot's fps.
    pub start_tick: i64,
    pub end_tick: i64,
    /// Source-in value at `source_rate` (the media's native rate).
    pub source_start: i64,
    pub source_rate: (i32, i32),
}

/// Every unmuted audio clip on the timeline + the sequence rate.
pub struct AudioSnapshot {
    pub fps: (i32, i32),
    pub spans: Vec<AudioSpan>,
}

enum AudioMsg {
    Snapshot(AudioSnapshot),
    /// Start (or re-anchor) producing from `tick`. `epoch` was assigned by
    /// the UI thread *before* send; blocks tagged older are already dead.
    Play { tick: i64, epoch: u64 },
    Pause,
}

/// One mixed block on its way to the device.
struct AudioBlock {
    epoch: u64,
    /// Interleaved stereo, `BLOCK_FRAMES * AUDIO_CHANNELS` long.
    samples: Vec<f32>,
}

// ---------------------------------------------------------------------------
// Shared clock state
// ---------------------------------------------------------------------------

struct AudioShared {
    /// Bumped by every play/seek; the callback discards stale-tagged blocks.
    epoch: AtomicU64,
    /// Timeline tick at the current epoch's origin.
    anchor_tick: AtomicI64,
    /// Device frames of the current epoch consumed by the callback.
    frames_played: AtomicU64,
    playing: AtomicBool,
    underruns: AtomicU64,
}

/// Cloneable, `Send` interface to the audio system: transport control for
/// the UI thread, snapshot publishing for the engine worker, and the master
/// clock for the playback timer.
#[derive(Clone)]
pub struct AudioHandle {
    shared: Arc<AudioShared>,
    tx: Option<Sender<AudioMsg>>,
    sample_rate: u32,
}

impl AudioHandle {
    /// Whether a real output device is pacing playback. False ⇒ the UI must
    /// fall back to the wall-clock transport.
    pub fn active(&self) -> bool {
        self.tx.is_some()
    }

    /// Start playing from `tick` (also the mid-playback seek: every seek is
    /// a re-anchored play). Clock state flips on the caller's thread, so a
    /// `current_tick` immediately after returns `tick` — no mixer round-trip.
    pub fn play(&self, tick: i64) {
        let epoch = self.shared.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.anchor_tick.store(tick, Ordering::Release);
        self.shared.frames_played.store(0, Ordering::Release);
        self.shared.playing.store(true, Ordering::Release);
        if let Some(tx) = &self.tx {
            let _ = tx.send(AudioMsg::Play { tick, epoch });
        }
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Release);
        if let Some(tx) = &self.tx {
            let _ = tx.send(AudioMsg::Pause);
        }
    }

    pub fn publish_snapshot(&self, snapshot: AudioSnapshot) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(AudioMsg::Snapshot(snapshot));
        }
    }

    /// Playhead tick by the audio clock: anchor + consumed device frames at
    /// the sequence rate (exact i128, floored).
    pub fn current_tick(&self, fps_num: i32, fps_den: i32) -> i64 {
        let anchor = self.shared.anchor_tick.load(Ordering::Acquire);
        if fps_num <= 0 || fps_den <= 0 || self.sample_rate == 0 {
            return anchor;
        }
        let frames = self.shared.frames_played.load(Ordering::Acquire);
        let ticks = i128::from(frames) * i128::from(fps_num)
            / (i128::from(self.sample_rate) * i128::from(fps_den));
        (i128::from(anchor) + ticks).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
    }
}

// ---------------------------------------------------------------------------
// System: device stream + mixer thread
// ---------------------------------------------------------------------------

/// Owns the cpal stream (`!Send` — lives on the main thread) and the mixer
/// thread. Dropping it stops audio; hand out [`AudioHandle`]s freely.
pub struct AudioSystem {
    handle: AudioHandle,
    _stream: Option<cpal::Stream>,
}

impl AudioSystem {
    /// Bring up the default output device. A machine without one degrades to
    /// a disabled system whose handle reports `active() == false`.
    pub fn start() -> Self {
        match Self::try_start() {
            Ok(system) => system,
            Err(e) => {
                warn!("audio output unavailable, playback clock falls back to wall time: {e}");
                Self {
                    handle: AudioHandle {
                        shared: Arc::new(AudioShared::new()),
                        tx: None,
                        sample_rate: 0,
                    },
                    _stream: None,
                }
            }
        }
    }

    pub fn handle(&self) -> AudioHandle {
        self.handle.clone()
    }

    fn try_start() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no default output device")?;
        let config = device
            .default_output_config()
            .map_err(|e| e.to_string())?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!(
                "default output format {:?} is not f32",
                config.sample_format()
            ));
        }
        let stream_config: cpal::StreamConfig = config.into();
        let sample_rate = stream_config.sample_rate;
        let device_channels = stream_config.channels as usize;

        let shared = Arc::new(AudioShared::new());
        let (msg_tx, msg_rx) = unbounded::<AudioMsg>();
        let (block_tx, block_rx) = bounded::<AudioBlock>(BLOCK_CAPACITY);

        let cb_shared = Arc::clone(&shared);
        let mut sink = CallbackSink::new(block_rx);
        let stream = device
            .build_output_stream(
                stream_config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    sink.fill(out, device_channels, &cb_shared);
                },
                |e| warn!("audio stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;

        let mixer_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("cutlass-audio-mixer".into())
            .spawn(move || mixer_loop(msg_rx, block_tx, mixer_shared, sample_rate))
            .map_err(|e| e.to_string())?;

        info!(sample_rate, device_channels, "audio output ready");
        Ok(Self {
            handle: AudioHandle {
                shared,
                tx: Some(msg_tx),
                sample_rate,
            },
            _stream: Some(stream),
        })
    }
}

impl AudioShared {
    fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            anchor_tick: AtomicI64::new(0),
            frames_played: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Device callback
// ---------------------------------------------------------------------------

/// Callback-side state: the block being consumed plus its read cursor.
struct CallbackSink {
    rx: Receiver<AudioBlock>,
    current: Option<AudioBlock>,
    cursor: usize,
}

impl CallbackSink {
    fn new(rx: Receiver<AudioBlock>) -> Self {
        Self {
            rx,
            current: None,
            cursor: 0,
        }
    }

    /// Real-time path: no locks, no blocking waits; allocation only when a
    /// stale block is dropped (acceptable for a desktop editor's callback).
    fn fill(&mut self, out: &mut [f32], device_channels: usize, shared: &AudioShared) {
        out.fill(0.0);
        if !shared.playing.load(Ordering::Acquire) {
            self.current = None;
            return;
        }
        let epoch = shared.epoch.load(Ordering::Acquire);
        let want_frames = out.len() / device_channels.max(1);
        let mut filled = 0usize;

        while filled < want_frames {
            let block = match &self.current {
                Some(b) if b.epoch == epoch && self.cursor < b.samples.len() => {
                    self.current.as_ref().expect("just matched")
                }
                _ => {
                    self.current = None;
                    match self.rx.try_recv() {
                        Ok(b) if b.epoch == epoch => {
                            self.cursor = 0;
                            self.current.insert(b)
                        }
                        Ok(_stale) => continue, // pre-seek block: drop, next
                        Err(_) => break,        // ring empty: underrun
                    }
                }
            };

            let frames_left = (block.samples.len() - self.cursor) / AUDIO_CHANNELS;
            let take = frames_left.min(want_frames - filled);
            for i in 0..take {
                let src = self.cursor + i * AUDIO_CHANNELS;
                let (l, r) = (block.samples[src], block.samples[src + 1]);
                let dst = (filled + i) * device_channels;
                match device_channels {
                    0 => {}
                    1 => out[dst] = 0.5 * (l + r),
                    _ => {
                        out[dst] = l;
                        out[dst + 1] = r;
                        // extra channels stay silent
                    }
                }
            }
            self.cursor += take * AUDIO_CHANNELS;
            filled += take;
        }

        if filled > 0 {
            shared
                .frames_played
                .fetch_add(filled as u64, Ordering::AcqRel);
        }
        if filled < want_frames {
            shared.underruns.fetch_add(1, Ordering::AcqRel);
        }
    }
}

// ---------------------------------------------------------------------------
// Mixer thread
// ---------------------------------------------------------------------------

/// A span with its times resolved to device sample frames.
struct ResolvedSpan {
    path: PathBuf,
    start_frame: i64,
    end_frame: i64,
    source_start_frame: i64,
}

fn mixer_loop(
    msg_rx: Receiver<AudioMsg>,
    block_tx: Sender<AudioBlock>,
    shared: Arc<AudioShared>,
    sample_rate: u32,
) {
    let mut spans: Vec<ResolvedSpan> = Vec::new();
    let mut pending_snapshot: Option<AudioSnapshot> = None;
    let mut fps = (0i32, 0i32);
    // Keyed by (path, timeline start): one reader per span, so sequential
    // playback never seeks, and two clips of the same media never thrash a
    // shared demuxer position.
    let mut readers: HashMap<(PathBuf, i64), AudioReader> = HashMap::new();
    let mut failed_opens: HashMap<PathBuf, ()> = HashMap::new();
    let mut epoch = 0u64;
    let mut playing = false;
    // Timeline device-frame position the next block mixes from.
    let mut write_frame = 0i64;
    let mut last_underruns = 0u64;

    loop {
        // Heartbeat doubles as the pacing valve: when the block channel is
        // full we sleep here instead of spinning.
        match msg_rx.recv_timeout(Duration::from_millis(4)) {
            Ok(msg) => {
                let mut latest = Some(msg);
                while let Some(msg) = latest.take() {
                    match msg {
                        AudioMsg::Snapshot(snapshot) => pending_snapshot = Some(snapshot),
                        AudioMsg::Play { tick, epoch: e } => {
                            epoch = e;
                            playing = true;
                            write_frame = ticks_to_frames(tick, fps, sample_rate);
                        }
                        AudioMsg::Pause => playing = false,
                    }
                    latest = msg_rx.try_recv().ok();
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }

        if let Some(snapshot) = pending_snapshot.take() {
            if playing && fps != snapshot.fps && fps.0 > 0 {
                // Rate change mid-play (project swap): keep the time, not
                // the frame count.
                let tick = frames_to_ticks(write_frame, fps, sample_rate);
                write_frame = ticks_to_frames(tick, snapshot.fps, sample_rate);
            }
            fps = snapshot.fps;
            spans = resolve_spans(&snapshot, sample_rate);
            let live: std::collections::HashSet<(PathBuf, i64)> = spans
                .iter()
                .map(|s| (s.path.clone(), s.start_frame))
                .collect();
            readers.retain(|key, _| live.contains(key));
            failed_opens.clear();
        }

        if !playing {
            continue;
        }

        // Fill whatever room the device side has left, but never starve the
        // message queue: a pending play/seek/snapshot outranks the next block.
        while !block_tx.is_full() && msg_rx.is_empty() {
            let mut samples = vec![0f32; BLOCK_FRAMES * AUDIO_CHANNELS];
            mix_block(
                &spans,
                &mut readers,
                &mut failed_opens,
                write_frame,
                sample_rate,
                &mut samples,
            );
            if block_tx
                .send(AudioBlock { epoch, samples })
                .is_err()
            {
                return; // device side gone
            }
            write_frame += BLOCK_FRAMES as i64;
        }

        let underruns = shared.underruns.load(Ordering::Acquire);
        if underruns > last_underruns {
            debug!(underruns, "audio underruns (device starved)");
            last_underruns = underruns;
        }
    }
}

/// Sum every span overlapping `[pos, pos + BLOCK_FRAMES)` into `out`.
fn mix_block(
    spans: &[ResolvedSpan],
    readers: &mut HashMap<(PathBuf, i64), AudioReader>,
    failed_opens: &mut HashMap<PathBuf, ()>,
    pos: i64,
    sample_rate: u32,
    out: &mut [f32],
) {
    let block_frames = (out.len() / AUDIO_CHANNELS) as i64;
    let block_end = pos + block_frames;
    let mut scratch = [0f32; BLOCK_FRAMES * AUDIO_CHANNELS];

    for span in spans {
        if span.start_frame >= block_end || span.end_frame <= pos {
            continue;
        }
        let s = span.start_frame.max(pos);
        let e = span.end_frame.min(block_end);
        let want = (e - s) as usize;

        let key = (span.path.clone(), span.start_frame);
        let reader = match readers.entry(key) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                if failed_opens.contains_key(&span.path) {
                    continue;
                }
                match AudioReader::open(&span.path, sample_rate) {
                    Ok(reader) => v.insert(reader),
                    Err(e) => {
                        warn!(path = %span.path.display(), "audio open failed: {e}");
                        failed_opens.insert(span.path.clone(), ());
                        continue;
                    }
                }
            }
        };

        let src_from = span.source_start_frame + (s - span.start_frame);
        if reader.seek_to_frame(src_from).is_err() {
            continue;
        }
        // A stream that starts after the requested point leaves a lead gap;
        // shift the mix-in to keep the rest aligned.
        let lead = reader
            .position()
            .map_or(0, |p| (p - src_from).clamp(0, e - s) as usize);

        let slots = &mut scratch[..want * AUDIO_CHANNELS];
        let got = match reader.read(&mut slots[lead * AUDIO_CHANNELS..]) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let offset = ((s - pos) as usize + lead) * AUDIO_CHANNELS;
        for (i, sample) in slots[lead * AUDIO_CHANNELS..(lead + got) * AUDIO_CHANNELS]
            .iter()
            .enumerate()
        {
            out[offset + i] += sample;
        }
    }

    for sample in out.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

fn resolve_spans(snapshot: &AudioSnapshot, sample_rate: u32) -> Vec<ResolvedSpan> {
    snapshot
        .spans
        .iter()
        .map(|span| ResolvedSpan {
            path: span.path.clone(),
            start_frame: ticks_to_frames(span.start_tick, snapshot.fps, sample_rate),
            end_frame: ticks_to_frames(span.end_tick, snapshot.fps, sample_rate),
            source_start_frame: ticks_to_frames(span.source_start, span.source_rate, sample_rate),
        })
        .collect()
}

/// `value` ticks at `rate` fps → device sample frames, exact i128, floored.
fn ticks_to_frames(value: i64, rate: (i32, i32), sample_rate: u32) -> i64 {
    let (num, den) = rate;
    if num <= 0 || den <= 0 || sample_rate == 0 {
        return 0;
    }
    let frames =
        i128::from(value) * i128::from(den) * i128::from(sample_rate) / i128::from(num);
    frames.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// Inverse of [`ticks_to_frames`] (floored).
fn frames_to_ticks(frames: i64, rate: (i32, i32), sample_rate: u32) -> i64 {
    let (num, den) = rate;
    if num <= 0 || den <= 0 || sample_rate == 0 {
        return 0;
    }
    let ticks = i128::from(frames) * i128::from(num)
        / (i128::from(den) * i128::from(sample_rate));
    ticks.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_frame_conversion_is_exact_for_integer_rates() {
        // 24 ticks (1s at 24fps) = 48000 frames at 48kHz.
        assert_eq!(ticks_to_frames(24, (24, 1), 48_000), 48_000);
        assert_eq!(frames_to_ticks(48_000, (24, 1), 48_000), 24);
        // One tick = 2000 frames.
        assert_eq!(ticks_to_frames(1, (24, 1), 48_000), 2_000);
    }

    #[test]
    fn tick_frame_conversion_handles_ntsc() {
        // 30000 ticks at 30000/1001 fps = 1001 seconds = 1001 · 48000 frames.
        assert_eq!(
            ticks_to_frames(30_000, (30_000, 1_001), 48_000),
            1_001 * 48_000
        );
        assert_eq!(
            frames_to_ticks(1_001 * 48_000, (30_000, 1_001), 48_000),
            30_000
        );
    }

    #[test]
    fn invalid_rates_collapse_to_zero() {
        assert_eq!(ticks_to_frames(100, (0, 1), 48_000), 0);
        assert_eq!(ticks_to_frames(100, (24, 1), 0), 0);
        assert_eq!(frames_to_ticks(100, (24, 0), 48_000), 0);
    }

    #[test]
    fn clock_reports_anchor_plus_consumed_frames() {
        let handle = AudioHandle {
            shared: Arc::new(AudioShared::new()),
            tx: None,
            sample_rate: 48_000,
        };
        handle.play(100);
        assert_eq!(handle.current_tick(24, 1), 100);
        // 1.5s of consumed audio = 36 ticks at 24fps.
        handle
            .shared
            .frames_played
            .store(72_000, Ordering::Release);
        assert_eq!(handle.current_tick(24, 1), 136);
    }

    #[test]
    fn mix_block_sums_and_clamps_silence_outside_spans() {
        // No spans: block stays silent.
        let mut readers = HashMap::new();
        let mut failed = HashMap::new();
        let mut out = vec![0.5f32; BLOCK_FRAMES * AUDIO_CHANNELS];
        mix_block(&[], &mut readers, &mut failed, 0, 48_000, &mut out);
        assert!(out.iter().all(|&s| s == 0.5), "no spans leave input alone");
    }

    fn audio_asset() -> Option<PathBuf> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.extension().is_some_and(|e| e == "mp3")
                    || (p.extension().is_some_and(|e| e == "mp4")
                        && AudioReader::open(p, 48_000).is_ok())
            })
    }

    #[test]
    fn mixer_renders_a_span_and_silence_around_it() {
        let Some(path) = audio_asset() else {
            return;
        };
        const RATE: u32 = 48_000;
        // One clip: timeline ticks [24, 48) at 24fps = seconds [1, 2),
        // playing the source from its start.
        let snapshot = AudioSnapshot {
            fps: (24, 1),
            spans: vec![AudioSpan {
                path,
                start_tick: 24,
                end_tick: 48,
                source_start: 0,
                source_rate: (24, 1),
            }],
        };
        let spans = resolve_spans(&snapshot, RATE);
        assert_eq!(spans[0].start_frame, i64::from(RATE));
        assert_eq!(spans[0].end_frame, 2 * i64::from(RATE));

        let mut readers = HashMap::new();
        let mut failed = HashMap::new();

        // Block fully before the clip: silence.
        let mut before = vec![0f32; BLOCK_FRAMES * AUDIO_CHANNELS];
        mix_block(&spans, &mut readers, &mut failed, 0, RATE, &mut before);
        assert!(before.iter().all(|&s| s == 0.0), "silence before the clip");

        // Block inside the clip: real audio.
        let mut inside = vec![0f32; BLOCK_FRAMES * AUDIO_CHANNELS];
        mix_block(
            &spans,
            &mut readers,
            &mut failed,
            i64::from(RATE) + 4 * BLOCK_FRAMES as i64,
            RATE,
            &mut inside,
        );
        assert!(
            inside.iter().any(|&s| s != 0.0),
            "audible inside the clip"
        );
        assert!(
            inside.iter().all(|&s| (-1.0..=1.0).contains(&s)),
            "clamped to [-1, 1]"
        );

        // Block straddling the clip start: leading samples stay silent.
        let straddle_pos = i64::from(RATE) - (BLOCK_FRAMES / 2) as i64;
        let mut straddle = vec![0f32; BLOCK_FRAMES * AUDIO_CHANNELS];
        mix_block(&spans, &mut readers, &mut failed, straddle_pos, RATE, &mut straddle);
        let lead = (BLOCK_FRAMES / 2) * AUDIO_CHANNELS;
        assert!(
            straddle[..lead].iter().all(|&s| s == 0.0),
            "silence before the in-point inside a straddling block"
        );
    }

    /// End-to-end mixer thread, no device: snapshot + play produce blocks
    /// whose epoch tags and silence/audio boundaries match the timeline;
    /// a re-anchored play (seek) switches epochs so stale blocks are
    /// identifiable for the callback's lock-free flush.
    #[test]
    fn mixer_thread_produces_tagged_blocks_and_reanchors() {
        let Some(path) = audio_asset() else {
            return;
        };
        const RATE: u32 = 48_000;
        let shared = Arc::new(AudioShared::new());
        let (msg_tx, msg_rx) = unbounded::<AudioMsg>();
        let (block_tx, block_rx) = bounded::<AudioBlock>(BLOCK_CAPACITY);
        let mixer_shared = Arc::clone(&shared);
        let mixer = std::thread::spawn(move || mixer_loop(msg_rx, block_tx, mixer_shared, RATE));

        // Clip covers timeline seconds [0, 2), source from 0.
        msg_tx
            .send(AudioMsg::Snapshot(AudioSnapshot {
                fps: (24, 1),
                spans: vec![AudioSpan {
                    path,
                    start_tick: 0,
                    end_tick: 48,
                    source_start: 0,
                    source_rate: (24, 1),
                }],
            }))
            .unwrap();
        msg_tx.send(AudioMsg::Play { tick: 0, epoch: 1 }).unwrap();

        // Drain one second of blocks: all epoch 1, audible.
        let mut heard = false;
        for _ in 0..(RATE as usize / BLOCK_FRAMES) {
            let block = block_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("mixer produces");
            assert_eq!(block.epoch, 1);
            heard |= block.samples.iter().any(|&s| s != 0.0);
        }
        assert!(heard, "the first second is audible");

        // Seek past the clip (tick 96 = 4s): fresh epoch, silent blocks.
        msg_tx.send(AudioMsg::Play { tick: 96, epoch: 2 }).unwrap();
        let mut saw_epoch2 = None;
        for _ in 0..(2 * BLOCK_CAPACITY + 4) {
            let block = block_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("mixer keeps producing");
            if block.epoch == 2 {
                saw_epoch2 = Some(block);
                break;
            }
        }
        let block = saw_epoch2.expect("epoch flips after the seek");
        assert!(
            block.samples.iter().all(|&s| s == 0.0),
            "past the clip the mixer paces with silence"
        );

        drop(msg_tx); // disconnect ⇒ mixer exits
        // Unblock a mixer stuck on a full block channel.
        while block_rx.try_recv().is_ok() {}
        mixer.join().expect("mixer thread exits cleanly");
    }
}
