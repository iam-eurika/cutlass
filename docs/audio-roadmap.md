# Audio suite roadmap — M8

Sound that doesn't need a DAW round-trip. This is the feature-area plan for
`v1-roadmap.md` § M8. The order is dependency-first: volume envelopes land
on the proven M2 `Param` plumbing (and unblock ducking, which is just
volume keyframes written by analysis), then the DSP-heavy pieces
(varispeed, denoise, beat detection) follow.

Policy reminder: **we follow CapCut.** The volume line + points, the
fade corner handles, varispeed with pitch lock, sidechain ducking, and
beat markers all mirror CapCut desktop's audio panel.

## Status legend

- [x] shipped
- [ ] not started / in progress

---

## Design (locked in Phase 1)

- **`volume` is a `Param<f32>` envelope**, not a bare gain
  (`cutlass-models/src/clip.rs`). One animation type, reused from M2:
  keyframe ticks are clip-relative timeline ticks, sampled with the same
  eased-lerp math as transforms. A constant envelope is the common case
  and serializes as the bare value (`"volume": 0.8`) — byte-identical to
  the pre-M8 shape, so old projects load unchanged and never-animated saves
  keep the old form. A keyframed envelope serializes as `{"kf":[...]}`.
- **Both mixers sample the envelope per sample-frame.** The shared
  `audio_gain_at(pos, len, &Param<f32>, fade_in, fade_out)` takes the
  envelope and multiplies the fades on top. Each mixer rebases the
  clip-relative *tick* keyframes into clip-relative *sample frames* once per
  span (`Param::map_ticks`), so the hot per-sample lookup stays an O(log k)
  tick compare, never a tick→frame conversion. The unity fast path
  (constant 1.0, no fades) still bypasses the gain loop entirely.
- **`set_clip_audio` sets a flat level** (CapCut's basic volume slider):
  it writes `Param::Constant(volume)`, flattening any envelope. Envelopes
  are drawn through the M2 keyframe commands routed to the new
  `ClipParam::Volume`, so ducking and the agent reuse the existing
  `SetParamKeyframe` / `RemoveParamKeyframe` / `SetParamConstant`
  vocabulary — no new command shapes, no new safety surface.
- **`ClipParam::Volume` is an audio property**: it bypasses the visual
  `check_param_target` (audio clips have no canvas placement to animate) and
  takes an audio-capable target check instead (media-backed; volume rides
  any media clip, since linkage lands the audible half on an audio lane).
  Values are validated in `0..=MAX_CLIP_VOLUME` per keyframe, finite.
- **A keyframed envelope is never "silent."** `Clip::is_silent` is true
  only for a constant gain of `0`; an envelope is kept by both mixers (it
  may be non-zero elsewhere) and sampled. Retimed clips still mute until
  varispeed (Phase 3).

---

## Phase 1 — Volume envelopes (the keystone)

- [x] **Model**: `Clip.volume: Param<f32>`; serde backward-compat
      (constant ⇔ bare value, keyframed ⇔ `{"kf":..}`); `Param::map_ticks`;
      envelope-aware `audio_gain_at`; `validate_volume` /
      `validate_volume_envelope`; `has_volume_envelope` / `is_silent`
      helpers; split rides the envelope on both halves.
- [x] **Engine / commands**: `ClipParam::Volume` routed at the project
      level to the clip's envelope with an audio-target check; the M2
      `SetParamKeyframe` / `RemoveParamKeyframe` / `SetParamConstant`
      actions drive it with their existing clip-snapshot inverses.
- [x] **Mixers**: the realtime mixer (`cutlass-ui/src/audio.rs`) and the
      export mixer (`cutlass-engine/src/export_audio.rs`) both carry the
      envelope on their span, rebase it to the sample-frame domain, and
      sample per frame; preview and export agree.
- [ ] **Agent vocabulary**: `volume` joins `WireClipParam` so the agent can
      write volume keyframes ("fade the music down under the voice"); wire
      DTO + validation + action-log phrasing + schema snapshot bump + eval.
- [ ] **Inspector envelope UI**: a keyframe diamond on the Volume row (the
      M2 cluster) plus the CapCut line-and-points envelope drawn on audio
      clips — drag a point to set gain, double-click to add/remove, with the
      projection publishing the volume curve like the transform curves.
- [ ] **Timeline badge**: the audio chip marks an enveloped clip
      (alongside the existing muted / volume% / fade chips).

## Phase 2 — Fades as corner handles

- [ ] Fade-in/out handles on the clip's top corners (sugar over the M1
      `fade_in`/`fade_out` fields, which already feed `audio_gain_at`);
      drag to set duration, with the existing badge.

## Phase 3 — Varispeed audio

- [ ] Time-stretch with pitch preservation so M1/M2 speed clips finally
      play sound (today retimed clips mute in both mixers). Pitch-shift
      toggle (chipmunk mode optional, as CapCut offers).
- [ ] **Open decision**: stretch backend (signalsmith-stretch vs
      rubberband vs a vendored phase-vocoder) — weigh quality, license, and
      bundling against the FFmpeg posture noted in the v1 roadmap. Drop the
      `is_retimed()` mixer mute once a backend lands.

## Phase 4 — Audio ducking

- [ ] Sidechain: detect speech-band energy on chosen "voice" lanes,
      auto-keyframe music lanes down (attack / release / threshold / amount).
      Writes **ordinary volume keyframes** (Phase 1 envelope) so ducking is
      inspectable and editable after the fact. One undoable history group.

## Phase 5 — Noise reduction

- [ ] rnnoise-class denoise as an audio effect on clips (offline render
      into the mixer path, cached like a proxy). **Open decision**: rnnoise
      binding vs ONNX model.

## Phase 6 — Beat detection & snap

- [ ] Onset analysis (local DSP) → beat markers on audio clips; "snap to
      beats" in the timeline magnet; the substrate for agent/M9 beat-sync.
      **Open decision**: aubio-style onset detector vs hand-rolled
      spectral-flux.

## Phase 7 — Audio scrub bursts

- [ ] Short audio bursts while dragging the playhead (the reserved
      `AudioReader` seam from `playback-roadmap.md` Phase 4).

## Phase 8 — MP3 frame-exact seek index

- [ ] Lazily-built MP3 seek index to kill the known tens-of-ms seek offset
      (`decoder` debt called out in the v1 roadmap).

---

## Exit

Music ducks under narration, denoised voice, beat-snapped cuts, audible
speed ramps — preview and export agree, every edit undoable.
