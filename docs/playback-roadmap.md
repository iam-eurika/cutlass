# Playback Roadmap — real-time transport, end to end

Policy: **we follow CapCut.** Space plays/pauses, the playhead glides, the
preview keeps up, audio is in sync, and edits during playback just work.
When a transport UX question comes up, the answer is "what does CapCut
desktop do?"

This doc tracks the path from today's scrub-only preview to that target.
Phases are ordered so each ships something usable on its own (same format
as `timeline-roadmap.md`).

## Architecture invariants (apply to every phase)

- **One transport, one gate.** Play/pause/seek route through
  `TimelineActions` — the same functions for the Space key, the preview
  panel button, and the fullscreen transport bar, so gating can never
  diverge (the Phase 5 timeline pattern).
- **The clock is truth while playing.** The playhead tick is always
  *computed* from the master clock at the sequence rate — never
  incremented per rendered frame. Slow decode drops frames (requests
  coalesce to the newest tick on the worker); it must never slow the
  playhead down. Since Phase 3 the master at speed 1/1 is the *audio
  device* (consumed sample frames); shuttle speeds and deviceless
  machines fall back to the scaled wall clock through the same
  `playback-tick` path.
- **Tick math is a pure Rust callback.** Anchor + elapsed → tick runs in
  `src/transport.rs` (exposed via `ui/lib/transport-backend.slint`) with
  exact i64 rational math — the resolver pattern every gesture uses.
- **Playback state is session state, not project state.** Playing/paused,
  the clock anchor, and (later) loop points live UI/worker-side; nothing
  here touches the engine's project, history, or the projection.
- **Decode and composite stay off the UI thread.** Playback renders
  through the existing worker `Frame` path (engine thread, coalesced).
  The UI thread only moves the playhead and swaps the delivered image.
- **Edits during playback are legal.** Every frame renders from the
  engine's current state; a mid-playback edit simply shows up on the
  next frame. Nothing pauses unless the user pauses.
- **Perf:** the per-frame UI work (timer step → tick set → frame request)
  is a hot path — allocation-free, O(1). Decode cost lives on the worker
  and is bounded by frame dropping, then attacked head-on in Phase 2.

## Status legend

- [x] shipped
- [ ] not started / in progress

---

## Phase 0 — Foundation (done)

What playback builds on, shipped by the timeline work:

- [x] Worker thread owns the engine; scrub `Frame(tick)` requests coalesce
      to the newest pending tick, mutations are never dropped
      (`src/preview_worker.rs`).
- [x] `Engine::get_frame`: decode → GPU composite → RGBA readback, with an
      on-disk YUV frame cache (50 GiB LRU) in front of the decoder.
- [x] Playhead/ruler/scrub UX: click/drag scrub, ←/→ frame step, Home/End,
      magnet snapping (timeline Phase 4).
- [x] Decoder groundwork: per-media `DecoderPool`, batch roll-forward
      decode (`video_strip`), audio decode to mono f32 via swresample
      (waveform peaks — the playback decode seam is reserved in
      `cutlass-decoder/src/audio/mod.rs`).

## Phase 1 — Silent playback transport ✅

The MVP: Space plays the timeline, video only. The clock lives UI-side
(a window-level `Timer` stepping the playhead from the wall clock); frames
ride the existing scrub path, so decode speed only affects smoothness,
never correctness.

- [x] Transport state: `TimelineStore.playing` + clock anchor; `play()` /
      `pause()` / `toggle-play()` / `playback-step()` in `TimelineActions`.
- [x] `TransportBackend.playback-tick` pure callback (`src/transport.rs`):
      anchor tick + elapsed ms → tick at the sequence rate, exact i64 math.
- [x] Window-level playback `Timer` (8ms, sub-frame): computes the tick,
      sets the playhead only when the frame index changes. The existing
      playhead watcher turns each change into a coalesced frame request —
      slower-than-realtime decode skips frames automatically.
- [x] The playhead→frame-request watcher moved from `TimelinePanel` to the
      window root: the panel unmounts in fullscreen preview mode, the
      transport must not.
- [x] Space toggles play/pause (window `FocusScope`, like all shortcuts);
      preview panel play button finally got a `TouchArea`; play/pause
      icon swap (new `pause.svg`).
- [x] CapCut end behavior: playback stops at the sequence end (playhead
      parks on the last tick); pressing play at the end restarts from 0.
      Empty timelines refuse to play. Edits that shrink the sequence
      under a playing playhead park-and-pause the same way.
- [x] Scrub/step/Home/End while playing = seek: the step detects an
      external playhead move and re-anchors the clock; playback continues
      from the new position.
- [x] Fullscreen preview joins the transport: live `PreviewStore.frame`
      instead of the static texture, play/pause button wired, seek slider
      follows the real playhead (watcher, not a `value` binding — the
      Slider's internal drag assignment would drop it) and seeks on drag.

## Phase 2 — Real-time decode performance

Phase 1 is honest but only as smooth as `get_frame`. Before this phase
every frame seeked (keyframe → re-decode the whole GOP prefix — O(GOP²)
work per GOP), so cold playback stuttered until the YUV cache warmed.
Sequential decode landed and made first-pass playback realtime.

- [x] Measure first: `playback_bench` example in `cutlass-engine`
      simulates sequential 24fps playback at three levels (decoder
      seek-per-frame, decoder roll-forward, engine `get_frame` e2e
      cold/warm); stage timings (`resolve`/`composite`, cache hit vs
      decode) emit as `tracing` debug events. Baseline on the M-series
      dev box, software decode: **seek-per-frame was the wall** —
      1080p24 48ms/frame (21fps), 4K 236–264ms/frame (~4fps); even
      cache-warm 1080p ran 32ms/frame because the cache *missed two
      ticks out of three* (see truncation fix below).
- [x] Sequential decode mode: `Decoder::frame_at` rolls forward from the
      decoder's last emitted frame when the target is ahead and inside
      the same GOP (the `video_strip` pattern, GOP-aware via
      `KeyframeIndex::gop_containing`); seeks only on discontinuities
      (backward target, GOP jump, fresh decoder) — byte-identical
      results to `seek_to_frame`, equivalence-tested against real media.
      Preview *and* export go through it (`decode_media_frame`).
      Measured: decoder 82x mean / 19–26x p95 faster sequentially;
      engine cold pass now **1080p24 3.0ms/frame (333fps), 4K24
      9.0ms/frame (111fps), 4K60 9.6ms/frame (104fps)** — all realtime
      at p95 with ~5x+ headroom, cache-cold.
- [x] Exact tick targets: the decode/cache path converted
      `RationalTime → Duration → stream ticks`, truncating twice — a
      rate-matched target landed one tick below the frame's stored PTS,
      so warm lookups missed ~2/3 of the time and seeks targeted a tick
      early. Replaced with `KeyframeIndex::rate_ticks_to_stream_ticks`
      (exact i128) + `frame_at_ticks`/`seek_ticks`; warm 1080p went
      32ms → 2.6ms/frame flat (max 3.0ms, was 71ms).
- [x] Keep the frame cache useful: playback fills it as it goes, and the
      writer was already non-blocking by design (dedicated thread,
      bounded channel, `try_send` drops frames instead of stalling
      decode, disk-pressure short-circuit) — verified, nothing to change.
- [x] Read-ahead: after each rendered frame, with the request queue idle,
      the worker warms decode + cache for the next 4 ticks
      (`Engine::prefetch` → `resolve_layers` without compositing). Stops
      the instant a new message arrives — the real request supersedes the
      guess — so the worker stays as responsive as before; a wrong guess
      (seek, reverse shuttle) only warms the cache. The GOP-boundary
      decode spike is paid during the idle gap *before* the cadence
      reaches it instead of hitching that frame.
- [ ] Explore: composite straight to a shared wgpu texture instead of
      RGBA readback → Slint image copy (Slint and the engine already
      share a wgpu 28 instance). Now the dominant steady-state cost
      (~6–7ms of the 4K frame is composite+readback+copy vs ~1–3ms
      decode); needs its own design pass — deliberately left out of the
      Phase 3/4 batch.

## Phase 3 — Audio playback & A/V sync ✅

The mixer the mute toggle has been waiting for (`Track.muted` was stored
and projected but honored by nothing). Three players, decoupled by
lock-free shared state (`src/audio.rs`): the UI thread owns transport
intent and reads the clock, a mixer thread owns decoders + the timeline
snapshot, and the device callback consumes mixed blocks.

- [x] Clocked, seekable audio decode in `cutlass-decoder` (the seam
      reserved in `audio/mod.rs`): `AudioReader` streams interleaved
      stereo f32 at the device rate from any source position, with PTS
      anchoring in exact i128, roll-forward for small forward gaps (the
      video `frame_at` philosophy), and a 200ms seek pre-roll so
      predictive codecs (MP3 bit reservoir, AAC) decode clean by the
      target. Sample-accuracy equivalence-tested against sequential
      decode on MP4/AAC.
- [x] Output device via `cpal`: one always-running stream; the callback
      pops mixed blocks from a bounded channel (6 × 1024 frames ≈ 128ms),
      spreads stereo onto the device channel count, and emits silence on
      underrun — it never blocks, never locks.
- [x] Mixer honors the project: the worker publishes an *audio snapshot*
      (every unmuted audio-lane clip: path, placement, source offset,
      rates) from `publish_projection` — the same chokepoint as the UI
      projection, so what playback sounds like can never diverge from
      what the timeline shows. Mute toggles, trims, moves, splits apply
      live mid-playback (worst case one buffer depth, ~128ms). Span times
      resolve to device sample frames in exact i128; readers are keyed
      per span so sequential playback never seeks.
- [x] **The audio clock is the master clock**: at speed 1/1 with a device
      present, `TransportBackend.playback-tick` answers from *consumed
      device frames* (anchor tick + frames/rate), not the wall clock —
      video chases audio by construction. The mixer produces zero-blocks
      for silent stretches, so silent timelines pace identically. No
      device ⇒ wall-clock fallback, silent.
- [x] Seek/pause/play stay atomically in sync via an *epoch counter*:
      play/seek bumps the epoch and resets the clock on the UI thread
      (no mixer round-trip), every block is tagged, and the callback
      drops stale-epoch blocks — a lock-free ring flush. Pause silences
      the callback instantly and freezes the clock (it counts only
      consumed frames).
- [x] Mute/hide toggles apply live during playback (snapshot republish;
      hide was already live through the engine's `!enabled` skip).

## Phase 4 — Transport polish

- [x] JKL shuttle: L forward (repeat doubles ×2/×4/×8), J the mirror in
      reverse, K pause — through `TimelineActions` like every transport
      path. Speed is a signed rational in `TimelineStore`;
      `playback-tick` scales the wall clock by it (exact i128, truncating
      toward the anchor in both directions). Audio gates to 1×: any other
      speed plays muted (varispeed audio later). Reverse rides Phase 2's
      decode work (seek-per-frame at ~3ms/frame 1080p — realtime even
      backwards; 4K reverse drops frames by design). K+L / K+J half-speed
      nudges later.
- [x] Loop playback toggle: toolbar button + Cmd/Ctrl+L; reaching the end
      bound wraps to the start bound (both directions) instead of
      park-and-pause. Play-around-playhead still open (CapCut "preview"
      affordance).
- [x] In/out range: I/O set marks at the playhead, Alt+X clears,
      ruler renders the band + edge pins; play confines to [in, out)
      (a playhead outside enters at the in-mark), loop loops it.
- [ ] Audio scrubbing (short sample bursts while dragging the playhead) —
      the `AudioReader` seam supports it; needs a scrub-burst path in the
      mixer.
- [ ] Playback resolution toggle (full/half/quarter preview decode):
      deferred — Phase 2 measurements say realtime holds through 4K60
      cache-cold, so there's no source that needs it yet.

---

## Known gaps / tech debt

- Phase 1's UI wall clock and the worker's render loop are decoupled by
  design (frame dropping), but there is no backpressure signal: a
  pathologically slow source renders seconds behind the playhead.
  Phase 2's sequential decode removed the systematic cause; read-ahead
  covers the residual GOP-boundary spike.
- MP3 mid-stream seeks are estimate-based (no sample table in the
  container), so a trimmed/mid-started MP3 clip can start up to a few
  tens of ms off its exact source offset. MP4/AAC is sample-accurate
  (tested). Frame-exact MP3 would need a parse-the-whole-file index —
  build it lazily if anyone notices.
- Audio comes only from audio lanes (by model design: imports with sound
  land a linked audio companion). A video clip whose companion was
  deleted, or dropped with linkage off, plays silent — consistent with
  the lanes the UI shows, but worth a "restore audio" affordance later.
- The audio clock counts only consumed frames, so a device-side underrun
  stalls the playhead for that buffer; with the mixer producing at
  >100× realtime this should never happen in practice (underruns are
  counted and logged at debug level — watch `audio underruns`).
- The mixer's seek flush is epoch-tagged block dropping in the callback;
  dropping a `Vec` deallocates in the realtime callback. Acceptable for
  a desktop editor (a few blocks per seek); a block-recycling pool is
  the fix if profiling ever flags it.
- Mid-playback edits become audible after the device buffer drains
  (~128ms). CapCut behaves the same; shrink `BLOCK_CAPACITY` if it ever
  feels laggy.
- JKL reverse plays video only and re-seeks per frame (backward targets
  always seek — `frame_at`'s roll-forward is forward-only). Realtime at
  1080p, frame-dropping at 4K. A backward GOP walk (decode GOP once,
  emit in reverse from a small frame stack) is the upgrade path if
  reverse becomes a daily tool.
- `frame_at` keeps no copy of the last emitted frame, so a *repeat*
  of the same target (or a target between two requests that resolve to
  one frame — timeline rate above source fps) re-seeks. Correct but
  slow; the worker's coalescing and the warm cache mask it today. If a
  sub-24fps source ever stutters during playback, add a kept-last-frame
  fast path (costs one frame copy per decode).
- Mismatched-rate cache keys: frames are stored under their decoded PTS
  but looked up by target tick, so a 60fps source on the 24fps timeline
  only hits the cache when the tick lands exactly on a frame PTS.
  Harmless for sequential playback (roll-forward is fast enough) but a
  warm re-scrub of such media still decodes. A PTS-quantizing lookup
  (nearest frame at or before the target) needs the cache index to
  answer range queries — `BTreeMap` instead of `HashMap`, measure first.
- `animation-tick()` drives the clock anchor in ms; fine for sessions
  under ~24 days (i32 ms). Revisit if Cutlass becomes a kiosk.
- The fullscreen preview's seek slider granularity is percent-of-duration;
  a real timecode scrubber (mini-ruler) is timeline-roadmap territory.
- Pause leaves the last delivered frame on screen; a paused seek re-renders
  through the scrub path (already correct today).
