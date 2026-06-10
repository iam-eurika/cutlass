//! Background preview rendering: engine and decode/composite stay off the UI thread.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use cutlass_commands::{Command, ProjectCommand};
use cutlass_engine::{ApplyOutcome, Engine, EngineConfig};
use cutlass_models::{Rational, RationalTime};
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
/// pending tick, but every [`WorkerMsg::Import`] is executed in order — it must
/// never be discarded by the coalescing drain.
fn worker_loop(
    engine: &mut Engine,
    tl_rate: Rational,
    preview_weak: slint::Weak<PreviewStore<'static>>,
    editor_weak: slint::Weak<EditorStore<'static>>,
    req_rx: Receiver<WorkerMsg>,
) {
    while let Ok(msg) = req_rx.recv() {
        match msg {
            WorkerMsg::Import(path) => import_and_publish(engine, &path, &editor_weak),
            WorkerMsg::Frame(mut tick) => {
                while let Ok(next) = req_rx.try_recv() {
                    match next {
                        WorkerMsg::Frame(latest) => tick = latest,
                        WorkerMsg::Import(path) => import_and_publish(engine, &path, &editor_weak),
                    }
                }
                render_frame(engine, tl_rate, &preview_weak, tick);
            }
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
