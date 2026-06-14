# Disk proxy cache — research & design

Status: **design, pre-implementation**. This records the investigation that
motivates an on-disk proxy/render cache, the measurements behind every decision,
and the planned architecture. All numbers were measured on the machine and
assets below; re-bench before trusting them on other hardware.

- **Machine:** Apple M5 Pro, 18 logical cores, VideoToolbox (HW decode + HW
  ProRes/H.264 encode). ffmpeg 8.x.
- **Primary asset:** `local-assets/assets/16078825_3840_2160_60fps.mp4` — 3840×2160, 59.94 fps,
  H.264, 36.5 s, GOP ≈ 250 frames (keyframe every ~4.17 s).
- **Bench harness:** `crates/cutlass-decode/benches/cold_seek.rs` (criterion, 100
  samples/case). Run with `CUTLASS_BENCH_ASSET=<file> cargo bench -p cutlass-decode --bench cold_seek`.
- Generated bench assets live in gitignored `target/bench-assets/`.

## 1. Motivation

We want **cache-on-import** so the timeline is responsive immediately, the way
CapCut and Filmora are. Two observations drove the investigation:

- Both apps use up to ~10 GB RAM while editing.
- Filmora's RAM later **drops to <2 GB but playback stays smooth** — strong hint
  that decoded/rendered content is being spilled to **disk**, not just held in RAM.

## 2. What the commercial apps actually do (on-disk inspection)

Measured under `~/Movies`:

| App | On-disk footprint | What it is |
|---|---|---|
| **Filmora** | **42 GB** in `Render/.Render/` | a real render cache |
| **CapCut** | 751 MB (591 MB `Cache/`) | effects, ML models, GPU shaders, thumbnails — **not** a frame cache |

**Filmora's `Render/.Render/`** is the mechanism we care about:

- Layout: `.Render/{device-hash}/{content-hash}/{0,1,2,…}.wsjpga` — numbered
  **segment** files, 300–500 MB each, 461 files total.
- `.wsjpga` is a **proprietary container** (ffprobe rejects it): a 16-byte
  content-hash tag, a header, then a **little-endian offset table**, then a
  sequence of standalone **JPEG frames** (confirmed: 261 `FF D8 FF E0 … JFIF`
  SOI markers in the first 200 MB of one segment).
- Effect: all-intra (every frame independently decodable) + offset index →
  **O(1) random seek**, cheap decode. Once a segment is on disk, the in-RAM
  decoded buffer can be released → the RAM drop we observed.

**CapCut** keeps no playback frame cache in `~/Movies`; its big RAM use stays in
memory (or a scratch dir elsewhere). So only Filmora's pattern is worth copying.

Takeaway: a disk cache = **all-intra + frame-indexed**, segmented, content-hashed,
with an eviction policy (Filmora's 42 GB shows what happens without a tight one).

## 3. The problem being solved: cold-seek latency

A *cold* seek = a jump into a freshly-opened decoder (no warm state): flush, enter
at the keyframe ≤ target, decode forward to the landing frame. This is what the
timeline hits on first scrub / backward / far-forward jumps. Cost scales with
frames-decoded-from-keyframe, so long-GOP sources are brutal.

Cold seek on the **4K60 H.264 source** (`seek_to_frame`, ms):

| Decode path | mid-clip | far (~90%) |
|---|---|---|
| auto (VideoToolbox HW) | 775 | **1565** |
| software | 362 | 634 |

**0.36–1.6 seconds per cold seek** — unusable scrubbing. (The HW number includes
the `av_hwframe_transfer_data` GPU→CPU readback for the landing frame, which the
compositor needs today; re-bench if it ever consumes GPU surfaces directly.)

## 4. The fix: a 1080p all-intra proxy, and its measured payoff

Re-encode each source to an all-intra (every-frame-a-keyframe) proxy at a
canonical **1080p** preview resolution. Seeking becomes "offset lookup + decode
one frame", independent of distance. Full 4K stays in the source for export.

Cold seek on the **1080p all-intra H.264 proxy** vs the 4K source:

| Path | source mid/far | proxy mid/far | speedup |
|---|---|---|---|
| auto (HW) | 775 / 1565 ms | 61.6 / 62.5 ms | 13–25× |
| **software** | 362 / 634 ms | **8.8 / 8.8 ms** | **41–72×** |

Two results:

1. **Proxy seek is flat** (mid ≈ far): the long-GOP worst-case cliff is gone.
2. **Software beats HW by ~7× on the proxy** (8.8 vs 62 ms) — VideoToolbox's
   fixed per-seek flush latency dominates when only one frame is decoded.

## 5. Codec choice — settled by measurement

The proxy's decoded pixel format **must** be one the engine accepts. Today
`crates/cutlass-decode/src/frame.rs` accepts only `YUV420P`, `NV12`, `RGBA`.

| Proxy codec | Decoded pixfmt | Supported? | Size (36.5 s @1080p) | Build speed |
|---|---|---|---|---|
| **all-intra H.264** (`g=1`) | yuv420p / NV12 | ✅ | **261 MB** | 4.6× RT |
| ProRes (VideoToolbox) | **P210LE** (10-bit 4:2:2) | ❌ (bench panicked) | 410 MB | 2.9× RT |
| MJPEG (Filmora's pick) | **yuvj420p** | ❌ | 1.1 GB | 6.5× RT |

**Decision: all-intra H.264.** Smallest on disk *and* the only option that decodes
to a supported format. ProRes/MJPEG would each require adding pixel-format support
first. Filmora chose MJPEG for cross-platform reasons we don't share. (261 MB /
36.5 s ≈ **0.43 GB/min** → a 10-min project ≈ 4.3 GB, vs Filmora's 42 GB.)

## 6. Build pipeline — decode is already parallel; encode was the bottleneck

Decode throughput of the 4K60 source (2190 frames, `ffmpeg -f null`):

| Path | fps | × realtime |
|---|---|---|
| HW (VideoToolbox) | 150 | 2.5× |
| SW, 1 thread | 76 | 1.3× |
| **SW, all 18 cores (frame threading)** | **465** | **7.75×** |

A *single* SW decode already saturates all cores and is **3× faster than HW**.

Full proxy build (decode + scale→1080p + encode), 36.5 s clip:

| Pipeline | wall | × realtime |
|---|---|---|
| HW decode + HW encode | 12.68 s | 2.9× |
| SW decode + SW ProRes (`prores_ks`) | 24.65 s | 1.5× |
| **SW decode + HW encode** | **5.34 s** | **6.84×** |
| (decode+scale only, no encode) | 5.09 s | 7.18× — the ceiling |

**Decision: build with SW frame-threaded decode → HW encode.** Encode is then
essentially free (5.34 vs 5.09 s); the pipeline is decode-bound and already uses
all CPU cores **and** the HW encode engine at once.

**Consequence for parallelism:** spawning N keyframe-split decoders does **not**
raise throughput for a single clip — one SW decode already uses every core, so N
would oversubscribe them. Slicing is still worthwhile, but for **playhead-first
priority** and **resumable/evictable segments**, not throughput.

## 7. Multi-import: SW and HW are independent engines

For the "import many clips, chew through all" case, running N×SW collides on cores
and starves the UI. Instead run **one SW lane + one HW lane** — different physical
engines. Measured (build clip A on SW lane, clip B on HW lane):

| | time |
|---|---|
| SW lane (A) alone | 5.33 s |
| SW lane (A) **while HW lane ran concurrently** | 5.59 s (**+5%**) |
| serial total (A then B) | 7.66 s |
| concurrent wall | 5.59 s (≈ longer lane, not the sum) |

The SW build barely noticed a second clip decoding on the HW block (+0.26 s),
because HW decode costs ~0 CPU. Concurrent wall ≈ `max(lanes)`. (Caveat: clip B
was small; with equal clips the win approaches "two videos in the time of one".)

## 8. The unifying rule: latency vs throughput, and activity-adaptive engines

- **Latency regime — live scrub of the active clip:** one stream of single-frame
  decodes → use **software** (≈2 ms warm / ≈9 ms cold vs ≈7/62 ms HW).
- **Throughput regime — background proxy builds:** **one SW lane + one HW lane**
  in parallel; dispatch the import queue across them.
- **Activity-adaptive engine choice** (resolves "reserve vs use HW"):
  - **Interacting:** live scrub on SW (low latency, a few cores); background leans
    on the **HW lane** *because it's ~free on CPU* and won't steal cores from live.
    HW decode's real value is **CPU offload during interaction**, not raw speed.
  - **Idle:** SW lane at full thread count + HW lane → max aggregate throughput.

## 9. Planned design (against `cutlass-engines`)

Flavor **A** (proxy file per media), with flavor B's segmentation kept for
priority/eviction. Source-frame proxy (no baked effects), so it shares the
existing `(MediaId, source_frame)` cache invariant and survives timeline edits.

- **Read path (the only hot-path change):** in `MediaPool::frame`, on a RAM-cache
  miss go **RAM → proxy reader (SW decode) → source reader (fallback)**. A ready
  proxy is just another `MediaReader` pointed at the proxy file.

  ```text
  RAM FrameCache (LRU, 256 MiB)  →  proxy .mov (SW, ~9 ms)  →  source (slow, fallback)
  ```

- **Per-media state:** `{ source: Box<dyn FrameReader>, proxy: Option<…>, status:
  ProxyStatus, last_access }`. `ProxyStatus = None | Building{pct} | Ready(path) | Failed`.
- **Import:** `import_media` stays fast (open source decoder + add to project) and
  calls `request_proxy(id)` to enqueue the background build; never blocks.
- **Build scheduler:** SW lane + HW lane (§8), keyframe-aligned segment jobs in a
  **playhead-priority** queue, work-stolen across lanes. SW lane thread-capped
  (leave ~2–4 cores) while the user interacts. Each worker owns its **own**
  decoder (re-opens the file) — never shares the live reader (ffmpeg `Send`).
- **Format:** all-intra H.264 (`g=1`), 1080p, HW-encoded. Keep segments + a
  `source_frame → (segment, frame)` index (parallel build produces slices anyway).
- **Keying:** `render_hash = blake3(source_content_id ++ proxy_resolution ++ codec
  ++ CACHE_VERSION)`. Content-addressed → shared across projects, survives
  restarts, safe to delete. Path: `~/Library/Caches/cutlass/proxies/<hash>/`.
- **Eviction:** `DiskBudget { cap_bytes }`, LRU over segments by `last_access`
  (mirror of `FrameCache::evict_to_fit`). Evicting a media's proxy resets
  `status = None`; it transparently falls back to the source and re-renders if
  scrubbed again. **Non-negotiable** — without it we recreate Filmora's 42 GB.

### Correctness note: open-GOP slice boundaries

Splitting build slices on keyframes is only safe at **IDR** boundaries. With
open-GOP H.264/HEVC, B-frames before the next keyframe can reference forward into
it, so a slice that stops exactly at `K_{i+1}` can't decode its tail correctly in
isolation. Mitigation: split only on IDR frames (extend `KeyframeIndex` to flag
IDR), or let a worker read a few packets past its boundary and discard spillover.
Camera/delivery codecs are usually closed-GOP, so this rarely bites — but it
produces garbled seam frames if ignored.

## 10. Implementation plan

- **P1 (shipped):** background build worker (SW-dec→HW-enc), all-intra-H.264
  proxy, reader swap in `MediaPool`, disk-LRU budget. Biggest win — kills cold
  seek for imports.
- **P2 (shipped):** lane pool over a shared **priority queue**, playhead
  priority, `Building { progress }` signal, activity pause. Details below.
- **P3 (optional):** custom segmented container with baked effects (a *timeline*
  render cache, true flavor B).

### P2 as built

- **Lanes + shared queue.** `ProxyService` spawns one thread per `LaneConfig`,
  all pulling from one `Queue` (`Mutex<Vec<RenderJob>>` + `Condvar`). Default mix
  is one **SW lane** (all cores — the fastest single pipeline) + one **HW-decode
  lane** (`HwAccel::Auto`, capped to 2 threads so a SW fallback can't
  oversubscribe). Whichever lane is free pops the highest-priority job, so
  multi-import builds in parallel and work-steals. Validated by
  `multiple_imports_build_in_parallel` (two clips → `≈max(lanes)` wall).
- **HW-decode build path.** `build_proxy_with` takes `ProxyBuildOptions { decode,
  decode_threads }`; a HW lane decodes the source on the GPU block and
  `transfer_to_cpu`s each surface before scale + HW encode. SW lanes skip the
  transfer. Covered by `proxy_builds_via_hardware_decode_lane`.
- **Playhead priority.** `Queue::prioritize` raises (never lowers) a *pending*
  job's priority; ties stay FIFO. `MediaPool::prioritize_proxy` /
  `Engine::set_playhead(frame)` bump the clip(s) under the playhead via a
  monotonic counter, so the footage on screen builds first.
- **Progress.** `build_proxy_with` invokes a progress callback per packet; the
  lane coalesces it to whole-percent `ProxyMsg::Progress`, and `poll_proxies`
  folds it into `ProxyStatus::Building { progress }` (without clobbering a
  Ready/Failed result that races a late message).
- **Activity throttle.** `Queue::set_paused` parks lanes between jobs;
  `Engine::set_background_paused(true)` during heavy scrub/playback keeps cycles
  on the live path, resume when idle. Drop signals shutdown (non-blocking; a
  lane finishes its in-flight build then exits).

New code: `crates/cutlass-engines/src/proxy.rs` (status, jobs, lanes, queue,
budget, hash); `crates/cutlass-decode/` gained the H.264-intra encoder +
downscale helper (`build_proxy_with`, `ProxyBuildOptions`).
Touch points: `pool.rs` (reader preference, `request_proxy`, completion pump,
priority/pause), `engine.rs` (`import_media` wiring, `set_playhead`,
`set_background_paused`).

### End-to-end measurement (import → wait → seek)

The `import_seek` bench drives the full path: import (build the proxy through the
pool/lane pool), then cold-seek the source vs the engine-built proxy. On the 4K60
asset (`16078825_3840_2160_60fps.mp4`, 36.5 s, 2190 frames, 251 MB):

| Stage | Result |
|---|---|
| Proxy build (the "little wait") | **8.06 s** → 64 MB 1080p all-intra (≈4× smaller) |
| Cold seek **mid (50%)** | source 780 ms → **proxy 3.17 ms** (~246×) |
| Cold seek **far (90%)** | source 1.538 s → **proxy 3.88 ms** (~397×) |

So after an ~8 s background wait, scrubbing flips from a 0.8–1.5 s stall per jump
to a flat ~3–4 ms — and the proxy seek stays flat regardless of distance. (These
are end-to-end through a freshly opened decoder per seek, so no RAM-cache hit
masks the seek; warm RAM-cache hits are ~0.) Reproduce:

```bash
CUTLASS_BENCH_ASSET="$PWD/local-assets/assets/16078825_3840_2160_60fps.mp4" \
  cargo bench -p cutlass-engines --bench import_seek
```

### Multi-import + random seek

The `multi_import_seek` bench imports several clips at once (parallel lane builds)
and then random-scrubs across all of them. On 4 × 4K clips (8–11.5 s each):

| Metric | Result |
|---|---|
| Serial build (one lane at a time) | 8.73 s (sum of solo builds) |
| **Parallel build (both lanes)** | **6.83 s wall → 1.28× speedup** |
| Random cold seek, **source** (HW-auto) | ~692 ms median (492–857 ms range) |
| Random cold seek, **proxy** (SW) | **~3.33 ms** (~**208×**) |

The seek win carries over to random multi-clip access — as expected, since the
proxy is all-intra so distance/clip don't matter. The **build** speedup is only
**1.28×, not ~2×**, and that's the interesting result: both lanes currently
converge on the *same* VideoToolbox **encoder** (a single fixed-function block),
and the SW lane already saturates all cores, so a second concurrent build is
encoder- and core-bound rather than free. §7's near-2× was decode-only with one
clip per lane; under real load the shared HW encoder is the ceiling.

**Implication / follow-up:** to get independent engines (the §8 principle) the
second lane should encode in **software (libx264)** while the first uses HW
encode — different engines, no contention. Worth benching before assuming more
lanes help. Reproduce:

```bash
CUTLASS_BENCH_N=4 cargo bench -p cutlass-engines --bench multi_import_seek
```

## 11. Open follow-ups

- **Live reader default:** today `DecodeOptions::default()` is HW-auto, but SW is
  better for scrub latency (§3, §8). Bench SW-vs-HW for the live path and likely
  flip the default for interactive decode.
- Quality of all-intra H.264 proxy at the chosen bitrate (used 60 Mbps in tests);
  tune for preview fidelity vs size.
- Re-bench the HW path if/when the compositor consumes GPU surfaces directly
  (removes the GPU→CPU readback from the seek cost).
- ~~Validate lane concurrency with two equally-sized large clips.~~ Done —
  `multiple_imports_build_in_parallel` (two copies of the test asset).
- **Segment-level priority/eviction (P3):** today priority and the disk budget
  are per-file; keyframe-aligned segment jobs would let the playhead region
  build/evict at finer granularity (needs the IDR-split work in §9).
- **Lane encoder contention:** `multi_import_seek` shows only 1.28× parallel
  build speedup — both lanes share the one VideoToolbox encoder. Try a HW-decode
  + **software-encode** (libx264) second lane (truly independent engines) and
  re-bench; gate lane count if it doesn't help.

## 12. Reproduction

```bash
# Generate the proxy used above (SW decode + HW encode, all-intra H.264, 1080p):
ffmpeg -i local-assets/assets/16078825_3840_2160_60fps.mp4 -vf scale=1920:1080 \
  -c:v h264_videotoolbox -g 1 -b:v 60M target/bench-assets/proxy_h264i.mov

# Cold-seek bench, source vs proxy:
CUTLASS_BENCH_ASSET="$PWD/local-assets/assets/16078825_3840_2160_60fps.mp4" \
  cargo bench -p cutlass-decode --bench cold_seek
CUTLASS_BENCH_ASSET="$PWD/target/bench-assets/proxy_h264i.mov" \
  cargo bench -p cutlass-decode --bench cold_seek

# Decode throughput HW vs SW:
ffmpeg -hwaccel videotoolbox -i <src> -f null -   # HW
ffmpeg -i <src> -f null -                         # SW (frame-threaded)
```
