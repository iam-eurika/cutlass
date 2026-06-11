# Preview Roadmap — interactive canvas, end to end

Policy: **we follow CapCut.** Select a clip and the player grows a bounding
box; drag the body to move it, corner handles to scale, the rotate affordance
to spin it; blue guides snap you to center; the inspector mirrors every value
numerically. When a spatial UX question comes up, the answer is "what does
CapCut desktop do?"

This doc tracks the path from today's display-only preview to that target.
Phases are ordered so each ships something usable on its own (same format as
`timeline-roadmap.md` / `playback-roadmap.md`).

## CapCut reference behavior (research notes)

- **Scale 100% = aspect-fit.** A clip at default transform is contain-fit
  inside the canvas, centered — never stretched. Mixed-aspect media letterboxes
  instead of distorting.
- **Selection follows the clip, not the panel.** Clicking a clip in the
  timeline *or* in the player selects it; both show the same selection (clip
  card highlight + preview bounding box).
- **Preview clicks pick the topmost element** under the cursor. There is no
  click-through to occluded layers — you select those on the timeline.
- **Gestures:** drag body = move; white corner handles = uniform scale about
  the center; rotate affordance below the box = rotation with snap at the
  cardinal angles. Arrow keys nudge position.
- **Guides:** dragging near the canvas center shows blue alignment lines
  (horizontal / vertical center) and snaps to them.
- **Inspector ("Basic" / Transform):** Position X/Y, Scale %, Rotation °,
  Opacity % — sliders plus numeric entry; double-click a label to reset that
  value; a diamond icon per property adds keyframes (keyframes are out of
  scope here, see "Later").

## Architecture invariants (apply to every phase)

- **The engine is the single source of truth.** Preview gestures end in a
  `cutlass_commands::EditCommand` applied on the worker thread; the UI
  re-renders from the republished projection. No Slint-side mutation of
  project state, ever.
- **One geometry, everywhere.** The placement math that draws a layer is the
  same math that hit-tests it and the same math that exports it: the engine
  computes a `LayerPlacement` per layer (`layer_placement` in
  `cutlass-engine/src/composite.rs`), the compositor draws exactly that, and
  hit-testing inverts exactly that. Picking can never disagree with rendering.
- **Resolvers are pure Rust callbacks.** Hit-tests, drag resolution, guide
  snapping live in `crates/cutlass-ui/src/` modules exposed through
  `ui/lib/*-backend.slint` pure callbacks — the `snap.rs` / `selection.rs`
  pattern. The gesture preview and the release commit read the same
  resolution.
- **Every command is undoable.** `SetClipTransform` has an inverse action;
  one gesture = one history entry (no per-frame undo spam).
- **Perf:** placement math runs per layer per frame (hot path) — closed-form,
  allocation-free. Hit-testing is a top-down point-in-rotated-rect scan over
  a handful of layers, O(layers) per click. Per-frame interactive transform
  preview must not enter the undo history or republish the projection.

## Status legend

- [x] shipped
- [ ] not started / in progress

---

## Phase 0 — Foundation (done)

What this builds on, shipped by the timeline/playback work:

- [x] Worker thread owns the engine; `Frame(tick)` requests coalesce;
      mutations republish the projection (`src/preview_worker.rs`).
- [x] GPU composite → RGBA readback → Slint `Image` preview, realtime
      through 4K60 (`playback-roadmap.md` Phase 2).
- [x] Selection state keyed by `ClipId` (`TimelineStore.selected-ids`,
      `src/selection.rs`) — preview selection reuses it as-is.
- [x] Generator rasters (text via cosmic-text, shapes via tiny-skia) cached
      per `(content, canvas)` (`src/generator_raster.rs`).
- [x] Letterbox aspect math exists in the fullscreen preview
      (`ui/fullscreen-preview.slint`).

## Phase 1 — Spatial transform foundation (engine) ✅

The 70% that isn't hit-testing: position/scale/rotation/opacity as model
state, flowing through composite *and* export. After this phase nothing is
interactive yet, but every layer can be placed, and the placement is
queryable.

- [x] `ClipTransform` on `Clip` (`cutlass-models`): `position` (content-center
      offset from canvas center, normalized to canvas dimensions so projects
      survive canvas-size changes; +y down), `scale` (uniform, 1.0 =
      aspect-fit), `rotation` (degrees, clockwise), `opacity` (0..=1).
      `#[serde(default)]` identity ⇒ old project files load unchanged.
- [x] `Project::set_transform` with validation (visual tracks only, finite
      values, positive scale, opacity 0..=1); `EditCommand::SetClipTransform`
      + undoable action (`RestoreClipAction` inverse, the `SetGenerator`
      pattern).
- [x] Compositor placement: `CompositeLayer` became content + placement
      (`LayerPlacement`: center px, pre-rotation size px, rotation, opacity).
      All three pipelines (solid / blit / yuv) draw a placed, rotated quad
      via a per-layer affine uniform instead of a fullscreen triangle;
      opacity multiplies content alpha in the shader. (Also retired the
      latent single-uniform-buffer clobber when two solid layers composited
      in one pass — regression-tested.) The canvas now clears to *opaque*
      black: placed layers can leave it uncovered, and preview/export define
      the background as black (matches the empty-timeline gap policy).
- [x] `resolve_layers` computes placement per layer via `layer_placement`
      (exported for Phase 2 hit-testing): media at decoded native size
      aspect-fit into the canvas (CapCut 100% — replaces the old
      stretch-to-canvas for mismatched aspect; the legacy CPU path's bilinear
      resize-to-canvas is gone, the GPU scales native-size uploads instead),
      generator rasters full-canvas, then the clip transform on top. Export
      inherits transforms for free (same resolve path).
- [x] Equivalence guard: identity transform on canvas-sized content renders
      the placed quad bit-exact with the old fullscreen-triangle output
      (existing YUV 1:1 sharpness/equivalence tests still pass unchanged).
- [x] Tests: model serde default + legacy-file deserialization + validation,
      command inverse round-trip, compositor placement (offset/rotation/
      opacity/two-solid pixels), aspect-fit math, end-to-end `get_frame`
      with a transformed solid.

## Phase 2 — Preview hit-testing & selection ✅

Click a clip in the player, see it selected — both ways.

- [x] Layer bounds without an engine round-trip — but cheaper than the
      planned worker-published snapshot: the projection now carries each
      clip's transform and native media dimensions (`media-width/height`,
      `transform-*` on the Slint `Clip`), and the UI computes placement on
      demand by calling the engine's own `layer_placement`. No new publish
      chokepoint, nothing recomputed on playhead moves, and the "one
      geometry" invariant holds by construction (same function, same crate).
      The projection's canvas size now mirrors the engine's even-rounding so
      the two can't drift by a pixel.
- [x] Preview ↔ canvas coordinate mapping as a pure callback
      (`src/preview_select.rs` behind `ui/lib/preview-backend.slint`): the
      docked panel's `ImageFit.contain` letterboxing inverted in Rust, one
      `contain_mapping` used by hit-test and outline drawing (and later
      gestures). Parameterized on view size, so the fullscreen preview can
      reuse it the day it needs picking (CapCut's fullscreen is
      playback-only, so it stays display-only for now).
- [x] Click in the preview selects the topmost layer whose rotated rect
      contains the point (CapCut: no click-through), walking lanes
      top-first and inverting the compositor's clockwise rotation. Routes
      through the same `SelectionBackend.select-clip` as a timeline click —
      link groups, primary anchor, and the inspector stay in sync. Click on
      empty canvas / letterbox bars deselects; Esc deselects (existing
      shortcut).
- [x] Selection overlay: the selected clip's placement quad stroked over
      the preview image (corners pre-rotated in Rust, drawn as a Slint
      `Path` — Slint can't rotate a `Rectangle`). Reactive bindings keep it
      following the playhead, edits, and panel resizes live; hidden when
      the selected clip isn't under the playhead or its lane is hidden.
- [x] Locked tracks don't hit-test (same rule as timeline selection);
      hidden lanes and not-yet-composited generators (sticker/effect/
      filter/adjustment) fall through to the layer below.
- [x] Tests (`src/preview_select.rs`): topmost-wins, locked/hidden/audio
      lanes skipped, playhead coverage, letterbox misses, transformed and
      rotated hit-tests, generator canvas-size hits, selection-box corner
      mapping with and without rotation, off-playhead/hidden invisibility.

## Phase 3 — Move gesture & guides ✅

The first direct manipulation: drag the selected clip around the canvas.

- [x] Drag resolver pure callback (`src/preview_gesture.rs` behind
      `PreviewBackend.resolve-drag`): press point + cursor delta → new
      normalized position, through the shared letterbox mapping — so content
      tracks the cursor 1:1 at any panel size. One resolution feeds the live
      preview *and* the release commit; the same gates as hit-testing
      (visual, enabled, unlocked, composited) decide draggability.
- [x] Live preview without history spam: `Engine::set_transform_override`
      holds one `(ClipId, ClipTransform)` of session state consulted by
      `resolve_layers` (export always passes `None`); the worker's
      `TransformOverride` messages coalesce to the newest value exactly like
      scrub frames, so a fast drag can't back the queue up behind stale
      composites. The projection stays frozen at press-time values — the
      selection box follows the gesture via an override position the panel
      threads into `selection-box`. Release commits one `SetClipTransform`
      (override cleared in the same worker step: no flicker, one history
      entry per gesture).
- [x] CapCut center guides: blue horizontal/vertical canvas-center lines,
      each axis magneting independently when the content center comes within
      the tolerance (6 viewport px, converted to canvas px in the resolver —
      the same place the commit position is computed).
- [x] Arrow keys nudge the selected clip in whole canvas px (Shift = 10)
      through the same commit path — one undoable entry per keypress, frame
      re-rendered immediately. When nothing nudgeable is selected (no
      selection, audio clip, locked/hidden lane) ←/→ keep their transport
      frame-step binding and ↑/↓ pass through.
- [x] No-op drags commit nothing (timeline Phase 2 rule) — including drags
      the center magnet returns exactly home, which read as unmoved.
- [x] Tests: engine override render + state/history isolation + clear
      restores committed output; resolver delta mapping, per-axis snap,
      snap-back-home no-op, press-time-position anchoring,
      locked/hidden/unknown rejection, nudge px→normalized math.

## Phase 4 — Scale & rotate handles ✅

- [x] Corner handles (white circles, CapCut-style) drag-scale uniformly
      about the content center: `resolve_scale` multiplies the committed
      scale by cursor-distance ÷ press-distance from the center (the grabbed
      corner tracks the cursor along its center ray), clamped at 5% so the
      box stays grabbable. Position/rotation/opacity pass through untouched.
- [x] Rotate affordance below the box (hollow ring, riding the rotation —
      its anchor comes pre-computed from `selection_box` as a constant
      viewport offset off the content's bottom edge): `resolve_rotate` adds
      the cursor's angular travel about the center to the committed
      rotation, magnets to 0/90/180/270 within 3°, and normalizes to
      (-180°, 180°]. A tooltip under the handle reads out whole degrees
      during the gesture.
- [x] Cursor feedback per handle (nwse/nesw-resize on corners, grab/grabbing
      on the ring, grabbing during a move) via a hover-zone squared-distance
      test against the same handle anchors the press dispatches on — so the
      cursor can't suggest a gesture the press wouldn't start. Handles render
      at constant UI px at any panel size/letterbox; a handle press never
      re-runs hit-testing (the box belongs to the selected clip).
- [x] Same override-then-commit flow as Phase 3 — one history entry per
      gesture, no-op gestures (scale back at the press distance, rotation
      magneted back to its committed angle) commit nothing. The selection
      box now follows the whole live resolution (position *and*
      scale/rotation), not just position.
- [x] Tests: scale ratio mapping, compounding off the committed scale, min
      clamp, press-at-pivot rejection, unmoved round-trips; rotation angle
      tracking, cardinal magnet, committed-rotation anchoring, half-turn
      normalization; rotate-handle anchor with and without rotation.

## Phase 5 — Inspector transform section ✅

Numeric truth for what the mouse does, CapCut's "Basic" block.

- [x] Transform section in the inspector for any visual clip (video, text,
      shape, solid; audio keeps its placeholder): Position X/Y (canvas px,
      displayed from normalized), Scale %, Rotation °, Opacity % — slider +
      numeric entry + unit suffix per row
      (`ui/panels/inspector/transform-inspector.slint`). No new backend:
      commits ride the same `EditorStore.on-clip-transform-committed` the
      preview gestures use, so the worker/undo path is shared by
      construction. Slider *motion* previews through the Phase 3 worker
      override (uncommitted, projection frozen) and slider *release* /
      numeric Enter commits one undoable `SetClipTransform`; releasing
      without a change (or Enter on the same value) commits nothing and
      just drops the override. Scale is floored at 1% (engine requires a
      positive scale), opacity clamped to 0–100%.
- [x] Double-click a property label to reset it to default (CapCut):
      position 0 px, scale 100%, rotation 0°, opacity 100% — each a normal
      single-property commit, no-op when already at the default.
- [x] Values update live, including mid-gesture: the in-flight resolution
      moved from panel-local state into `PreviewStore.gesture` (+ active
      flag and clip id), written by whoever drives the gesture — preview
      move/scale/rotate drags *and* inspector sliders. The selection box
      and the inspector rows both read it, so dragging a slider moves the
      box in the player and dragging the clip spins the inspector numbers.
      Slider/LineEdit widgets self-assign on interaction (which drops plain
      bindings), so external updates re-sync through a `changed value`
      watcher instead.

---

## Later / out of scope for this roadmap

- **Keyframes** (CapCut's diamond icons — animated transforms). Needs a
  keyframe model + interpolation in `resolve_layers`; designed after static
  transforms ship.
- **Crop** (CapCut crop tool reframes content within the box).
- **Non-uniform scale / flip** (CapCut exposes Mirror separately).
- **Multi-select group transform** in the preview.
- **Text re-raster at effective size**: text/shape rasters are canvas-sized;
  scaling above 100% interpolates (slight blur). Fix is rastering at the
  transformed pixel size with cache keys including scale — measure first.

## Known gaps / tech debt

- Aspect-fit at scale 1.0 intentionally changes output for mixed-aspect
  media that the old path stretched to the canvas (CapCut parity; the old
  behavior distorted).
- Per-layer uniform buffers are created per draw (a handful per frame, like
  the existing per-draw textures/bind groups); pool them if profiling ever
  flags allocation pressure.
- Solid layers accept transforms (they become placed colored rects). CapCut
  treats backgrounds as full-canvas; ours is a harmless superset.
