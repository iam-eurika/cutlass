//! Background preview rendering: engine and decode/composite stay off the UI thread.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use cutlass_commands::{Command, EditCommand, EditOutcome, ProjectCommand};
use cutlass_engine::{ApplyOutcome, Engine, EngineConfig};
use cutlass_models::{
    ClipId, MediaId, Rational, RationalTime, TimeRange, Track, TrackId, TrackKind, resample,
};
use tracing::{error, info};

use crate::{EditorStore, PreviewStore};

pub struct PreviewSession {
    pub duration_ticks: i64,
    pub tl_rate: Rational,
}

/// Work submitted to the engine thread. Scrub frames coalesce to the latest
/// pending tick; imports must not be dropped by that coalescing (see
/// [`worker_loop`]).
enum WorkerMsg {
    Frame(i64),
    Import(PathBuf),
    /// Place the full range of `media` (raw id from the Slint projection) at
    /// `start_tick` sequence ticks. `track` is the targeted video lane's raw
    /// id, or empty to create a new video lane at `drop_row` (the lane-list
    /// row under the cursor, top-first; may be out of range).
    AddClip {
        media: String,
        track: String,
        start_tick: i64,
        drop_row: i64,
    },
    /// Move `clip` (raw id) to `track` at `start_tick`, or — when `track` is
    /// empty — to a new lane of the clip's kind inserted at `insert_row`.
    MoveClip {
        clip: String,
        track: String,
        insert_row: i64,
        start_tick: i64,
    },
    /// Re-place `clip` (raw id) at `[start_tick, start_tick + duration_ticks)`
    /// on its own lane (edge trim; the engine re-derives the source in/out).
    TrimClip {
        clip: String,
        start_tick: i64,
        duration_ticks: i64,
    },
}

/// Cheap, cloneable sender to the engine thread. Hand one clone to each UI
/// callback that needs to talk to the engine (scrub, import, …). Cloning keeps
/// the channel — and therefore the worker loop — alive.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: Sender<WorkerMsg>,
}

impl WorkerHandle {
    pub fn request_frame(&self, tick: i64) {
        let _ = self.tx.send(WorkerMsg::Frame(tick));
    }

    pub fn import(&self, path: PathBuf) {
        let _ = self.tx.send(WorkerMsg::Import(path));
    }

    pub fn add_clip(&self, media: String, track: String, start_tick: i64, drop_row: i64) {
        let _ = self.tx.send(WorkerMsg::AddClip {
            media,
            track,
            start_tick,
            drop_row,
        });
    }

    pub fn move_clip(&self, clip: String, track: String, insert_row: i64, start_tick: i64) {
        let _ = self.tx.send(WorkerMsg::MoveClip {
            clip,
            track,
            insert_row,
            start_tick,
        });
    }

    pub fn trim_clip(&self, clip: String, start_tick: i64, duration_ticks: i64) {
        let _ = self.tx.send(WorkerMsg::TrimClip {
            clip,
            start_tick,
            duration_ticks,
        });
    }
}

pub struct PreviewWorker {
    handle: WorkerHandle,
    _join: JoinHandle<()>,
}

impl PreviewWorker {
    /// Spawns a dedicated thread that owns the [`Engine`] (required: decoders are not `Send`).
    pub fn spawn(
        config: EngineConfig,
        preview_weak: slint::Weak<PreviewStore<'static>>,
        editor_weak: slint::Weak<EditorStore<'static>>,
    ) -> Result<(Self, PreviewSession), String> {
        let (ready_tx, ready_rx) = bounded(1);
        let (req_tx, req_rx) = unbounded();

        let join = std::thread::Builder::new()
            .name("cutlass-preview".into())
            .spawn(move || {
                if let Err(e) = worker_main(config, preview_weak, editor_weak, req_rx, ready_tx) {
                    error!("preview worker exited: {e}");
                }
            })
            .map_err(|e| e.to_string())?;

        let session = ready_rx
            .recv()
            .map_err(|e| e.to_string())?
            .map_err(|e: String| e)?;

        Ok((
            Self {
                handle: WorkerHandle { tx: req_tx },
                _join: join,
            },
            session,
        ))
    }

    /// Clone a sender for a UI callback.
    pub fn handle(&self) -> WorkerHandle {
        self.handle.clone()
    }
}

fn worker_main(
    config: EngineConfig,
    preview_weak: slint::Weak<PreviewStore<'static>>,
    editor_weak: slint::Weak<EditorStore<'static>>,
    req_rx: Receiver<WorkerMsg>,
    ready_tx: Sender<Result<PreviewSession, String>>,
) -> Result<(), String> {
    // Start from an empty project: media arrives via user-driven imports
    // (Library → engine), not a hardcoded bootstrap asset.
    let mut engine = Engine::new(config).map_err(|e| e.to_string())?;
    let timeline = engine.project().timeline();
    let session = PreviewSession {
        duration_ticks: timeline.duration().value,
        tl_rate: timeline.frame_rate,
    };
    info!(
        duration_ticks = session.duration_ticks,
        tl_rate = ?session.tl_rate,
        "preview worker ready (empty project)"
    );
    let tl_rate = session.tl_rate;
    ready_tx
        .send(Ok(session))
        .map_err(|e| e.to_string())?;

    // Seed the UI with the engine's project so the editor reads from the engine
    // from the first frame (rather than any Slint-side placeholder).
    publish_projection(&engine, &editor_weak);

    worker_loop(&mut engine, tl_rate, preview_weak, editor_weak, req_rx);
    Ok(())
}

/// Single consumer for the engine thread. Scrub frames coalesce to the latest
/// pending tick, but every mutation (import, add-clip, move-clip, …) is
/// executed in order — it must never be discarded by the coalescing drain.
fn worker_loop(
    engine: &mut Engine,
    tl_rate: Rational,
    preview_weak: slint::Weak<PreviewStore<'static>>,
    editor_weak: slint::Weak<EditorStore<'static>>,
    req_rx: Receiver<WorkerMsg>,
) {
    let mutate = |engine: &mut Engine, msg: WorkerMsg| match msg {
        WorkerMsg::Import(path) => import_and_publish(engine, &path, &editor_weak),
        WorkerMsg::AddClip {
            media,
            track,
            start_tick,
            drop_row,
        } => add_clip_and_publish(engine, &media, &track, start_tick, drop_row, &editor_weak),
        WorkerMsg::MoveClip {
            clip,
            track,
            insert_row,
            start_tick,
        } => move_clip_and_publish(engine, &clip, &track, insert_row, start_tick, &editor_weak),
        WorkerMsg::TrimClip {
            clip,
            start_tick,
            duration_ticks,
        } => trim_clip_and_publish(engine, &clip, start_tick, duration_ticks, &editor_weak),
        WorkerMsg::Frame(_) => unreachable!("frames are handled by the drain below"),
    };

    while let Ok(msg) = req_rx.recv() {
        match msg {
            WorkerMsg::Frame(mut tick) => {
                while let Ok(next) = req_rx.try_recv() {
                    match next {
                        WorkerMsg::Frame(latest) => tick = latest,
                        other => mutate(engine, other),
                    }
                }
                render_frame(engine, tl_rate, &preview_weak, tick);
            }
            other => mutate(engine, other),
        }
    }
}

fn import_and_publish(
    engine: &mut Engine,
    path: &Path,
    editor_weak: &slint::Weak<EditorStore<'static>>,
) {
    match engine.apply(Command::Project(ProjectCommand::Import {
        path: path.to_path_buf(),
    })) {
        Ok(ApplyOutcome::Imported { media }) => {
            info!(
                ?media,
                path = %path.display(),
                pool = engine.project().media_count(),
                "imported media into pool"
            );
            publish_projection(engine, editor_weak);
        }
        Ok(other) => error!(path = %path.display(), "unexpected import outcome: {other:?}"),
        Err(e) => error!(path = %path.display(), "import failed: {e}"),
    }
}

/// Place the full source range of `media` on a video track, then republish the
/// projection so the clip appears.
///
/// Placement policy (CapCut-ish):
/// - dropped on a video lane → that lane, sliding right into the first gap
///   that fits when the drop tick overlaps existing clips;
/// - dropped on empty timeline space (`track` empty) → a fresh video track
///   inserted at `drop_row`, so the new lane appears where the user dropped
///   (above the lanes ⇒ top of the stack, below ⇒ bottom).
fn add_clip_and_publish(
    engine: &mut Engine,
    media: &str,
    track: &str,
    start_tick: i64,
    drop_row: i64,
    editor_weak: &slint::Weak<EditorStore<'static>>,
) {
    let Some(media_id) = parse_raw_id(media).map(MediaId::from_raw) else {
        error!(media, "drop ignored: unparsable media id");
        return;
    };
    let Some(source) = engine.project().media(media_id).map(|m| m.full_range()) else {
        error!(%media_id, "drop ignored: media not in pool");
        return;
    };
    let tl_rate = engine.project().timeline().frame_rate;
    // Mirror Project::add_clip's source→timeline resampling so first-fit sees
    // the same extent the engine will validate.
    let duration_ticks = resample(source.duration, tl_rate).value.max(1);
    let desired = start_tick.max(0);

    let (track_id, start_value) = match video_lane(engine, track) {
        Some(lane) => {
            let lane_track = engine
                .project()
                .timeline()
                .track(lane)
                .expect("video_lane returned an existing track");
            (lane, first_fit_start(lane_track, desired, duration_ticks))
        }
        None => match create_track(engine, TrackKind::Video, drop_row) {
            Ok(id) => (id, desired),
            Err(e) => {
                error!(%media_id, "drop failed creating video track: {e}");
                return;
            }
        },
    };

    match engine.apply(Command::Edit(EditCommand::AddClip {
        track: track_id,
        media: media_id,
        source,
        start: RationalTime::new(start_value, tl_rate),
    })) {
        Ok(ApplyOutcome::Edited(EditOutcome::Created(clip))) => {
            info!(
                %clip, %track_id, %media_id,
                start_tick = start_value,
                desired,
                "added clip from library drop"
            );
            publish_projection(engine, editor_weak);
        }
        Ok(other) => error!(%media_id, "unexpected add-clip outcome: {other:?}"),
        // First-fit should have made the placement valid; the engine still
        // rejects atomically if not, so just surface the reason.
        Err(e) => error!(%media_id, %track_id, start_tick = start_value, "add clip failed: {e}"),
    }
}

/// `track` (raw id from the Slint projection) when it names an existing video lane.
fn video_lane(engine: &Engine, track: &str) -> Option<TrackId> {
    let id = TrackId::from_raw(parse_raw_id(track)?);
    engine
        .project()
        .timeline()
        .track(id)
        .is_some_and(|t| t.kind == TrackKind::Video)
        .then_some(id)
}

/// Move a dragged clip to its resolved landing spot: an existing lane
/// (`track` set) or a new lane of the clip's kind inserted at `insert_row`.
/// A cross-lane move that empties its source lane removes that lane
/// (CapCut deletes overlay tracks that empty out).
fn move_clip_and_publish(
    engine: &mut Engine,
    clip: &str,
    track: &str,
    insert_row: i64,
    start_tick: i64,
    editor_weak: &slint::Weak<EditorStore<'static>>,
) {
    let Some(clip_id) = parse_raw_id(clip).map(ClipId::from_raw) else {
        error!(clip, "move ignored: unparsable clip id");
        return;
    };
    let Some(source_track) = engine.project().timeline().track_of(clip_id) else {
        error!(%clip_id, "move ignored: clip not on the timeline");
        return;
    };
    let kind = engine
        .project()
        .timeline()
        .track(source_track)
        .expect("track_of returned an existing track")
        .kind;
    let tl_rate = engine.project().timeline().frame_rate;

    let mut created_lane = false;
    let to_track = match parse_raw_id(track).map(TrackId::from_raw) {
        Some(id) => id,
        None => match create_track(engine, kind, insert_row) {
            Ok(id) => {
                created_lane = true;
                id
            }
            Err(e) => {
                error!(%clip_id, "move failed creating {kind:?} track: {e}");
                return;
            }
        },
    };

    match engine.apply(Command::Edit(EditCommand::MoveClip {
        clip: clip_id,
        to_track,
        start: RationalTime::new(start_tick.max(0), tl_rate),
    })) {
        Ok(ApplyOutcome::Edited(EditOutcome::Updated(_))) => {
            info!(%clip_id, %to_track, start_tick, "moved clip");
            remove_track_if_empty(engine, source_track, to_track);
            publish_projection(engine, editor_weak);
        }
        Ok(other) => error!(%clip_id, "unexpected move-clip outcome: {other:?}"),
        // The drag resolver previewed a valid spot; the engine still rejects
        // atomically if the projection raced a concurrent edit.
        Err(e) => {
            error!(%clip_id, %to_track, start_tick, "move clip failed: {e}");
            // Don't leave a lane we just created lingering empty.
            if created_lane {
                remove_track_if_empty(engine, to_track, source_track);
            }
            publish_projection(engine, editor_weak);
        }
    }
}

/// Re-place a trimmed clip at its resolved extent. The trim resolver already
/// clamped to neighbors and source headroom, so this should always apply; the
/// engine still validates atomically (overlap, source bounds) and we surface
/// any rejection rather than mutating the projection optimistically.
fn trim_clip_and_publish(
    engine: &mut Engine,
    clip: &str,
    start_tick: i64,
    duration_ticks: i64,
    editor_weak: &slint::Weak<EditorStore<'static>>,
) {
    let Some(clip_id) = parse_raw_id(clip).map(ClipId::from_raw) else {
        error!(clip, "trim ignored: unparsable clip id");
        return;
    };
    let tl_rate = engine.project().timeline().frame_rate;
    let timeline = TimeRange::at_rate(start_tick.max(0), duration_ticks.max(1), tl_rate);

    match engine.apply(Command::Edit(EditCommand::TrimClip {
        clip: clip_id,
        timeline,
    })) {
        Ok(ApplyOutcome::Edited(EditOutcome::Updated(_))) => {
            info!(%clip_id, start_tick, duration_ticks, "trimmed clip");
            publish_projection(engine, editor_weak);
        }
        Ok(other) => error!(%clip_id, "unexpected trim-clip outcome: {other:?}"),
        Err(e) => error!(%clip_id, start_tick, duration_ticks, "trim clip failed: {e}"),
    }
}

/// Remove `track` when a move left it empty (skipped if it's the lane the
/// clip just landed on).
fn remove_track_if_empty(engine: &mut Engine, track: TrackId, just_used: TrackId) {
    if track == just_used {
        return;
    }
    let emptied = engine
        .project()
        .timeline()
        .track(track)
        .is_some_and(|t| t.is_empty());
    if !emptied {
        return;
    }
    if let Err(e) = engine.apply(Command::Edit(EditCommand::RemoveTrack { track })) {
        error!(%track, "failed to remove emptied track: {e}");
    }
}

/// Create a new track of `kind` for drops/moves that don't target an existing
/// lane, inserted so it appears at `drop_row` in the lane list. Named by
/// kind + per-kind count (V1, V2, A1, …).
fn create_track(engine: &mut Engine, kind: TrackKind, drop_row: i64) -> Result<TrackId, String> {
    let timeline = engine.project().timeline();
    // The lane list shows the stack top-first (see projection.rs), so the new
    // lane appears at UI row r when inserted at stack index (len - r). The
    // clamp covers drops above the first lane (⇒ top of stack) and below the
    // last (⇒ bottom).
    let stack_len = timeline.order().len() as i64;
    let order_index = (stack_len - drop_row).clamp(0, stack_len) as usize;
    let count = timeline.tracks_ordered().filter(|t| t.kind == kind).count();
    match engine.apply(Command::Edit(EditCommand::AddTrack {
        kind,
        name: format!("{}{}", kind_prefix(kind), count + 1),
        index: Some(order_index),
    })) {
        Ok(ApplyOutcome::Edited(EditOutcome::CreatedTrack(id))) => Ok(id),
        Ok(other) => Err(format!("unexpected add-track outcome: {other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

fn kind_prefix(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "V",
        TrackKind::Audio => "A",
        TrackKind::Text => "T",
        TrackKind::Sticker => "ST",
        TrackKind::Effect => "FX",
        TrackKind::Filter => "F",
        TrackKind::Adjustment => "ADJ",
    }
}

/// First start ≥ `desired` where `[start, start + duration)` fits in a gap on
/// `track`. Clips are scanned in start order (they never overlap), so a blocked
/// candidate just slides to the blocker's end — O(n) on this cold per-drop path.
fn first_fit_start(track: &Track, desired: i64, duration_ticks: i64) -> i64 {
    let mut start = desired;
    for clip in track.clips_ordered() {
        if start + duration_ticks <= clip.timeline.start.value {
            break; // fits entirely before this clip
        }
        start = start.max(clip.timeline.end_tick());
    }
    start
}

fn parse_raw_id(raw: &str) -> Option<u64> {
    raw.parse().ok()
}

/// Snapshot the engine's project and hand it to the UI thread, which rebuilds
/// the Slint view model. The snapshot crosses the thread boundary (`Send`);
/// the `!Send` Slint model types are constructed inside the event-loop closure.
fn publish_projection(engine: &Engine, editor_weak: &slint::Weak<EditorStore<'static>>) {
    let project = engine.project().clone();
    let editor_weak = editor_weak.clone();
    if let Err(e) = slint::invoke_from_event_loop(move || {
        if let Some(store) = editor_weak.upgrade() {
            store.set_project(crate::projection::project_to_slint(&project));
        }
    }) {
        error!("failed to publish project projection to UI: {e}");
    }
}

fn render_frame(
    engine: &mut Engine,
    tl_rate: Rational,
    preview_weak: &slint::Weak<PreviewStore<'static>>,
    tick: i64,
) {
    match engine.get_frame(RationalTime::new(tick, tl_rate)) {
        Ok(frame) => {
            let weak = preview_weak.clone();
            if let Err(e) = slint::invoke_from_event_loop(move || {
                if let Some(store) = weak.upgrade() {
                    store.set_frame(crate::preview::to_slint_image(frame));
                }
            }) {
                error!("failed to deliver preview frame to UI: {e}");
            }
        }
        Err(e) => error!(tick, "preview frame failed: {e}"),
    }
}
