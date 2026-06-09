//! End-to-end session smoke test: new project → import → edit → save → export.
//!
//! Writes under `.cutlass/` in the current working directory:
//!   - `projects/<name>.cutlass` — saved project
//!   - `exports/<name>.mp4` — rendered timeline you can open in any player
//!   - `cache/` — frame cache (preview path; export decodes originals)

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cutlass_commands::{Command, EditCommand, EditOutcome, ProjectCommand};
use cutlass_engine::{ApplyOutcome, Engine, EngineConfig};
use cutlass_models::{RationalTime, TimeRange, TrackKind};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

type AnyError = Box<dyn Error + Send + Sync>;

const ASSETS_DIR: &str = "assets";
const CUTLASS_DIR: &str = ".cutlass";
const PROJECTS_SUBDIR: &str = "projects";
const EXPORTS_SUBDIR: &str = "exports";
const CACHE_SUBDIR: &str = "cache";
const MAX_EXPORT_FRAMES: i64 = 96;

struct Args {
    video: PathBuf,
    name: String,
}

fn setup_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

fn parse_args() -> Result<Args, AnyError> {
    let mut video: Option<PathBuf> = None;
    let mut name = env::var("CUTLASS_NAME").unwrap_or_else(|_| "demo".into());
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--name" | "-n" => {
                name = iter.next().ok_or("missing value after --name")?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag: {other}").into());
            }
            path => video = Some(PathBuf::from(path)),
        }
    }

    let video = match video {
        Some(p) => p,
        None => first_asset_in(ASSETS_DIR)
            .ok_or_else(|| format!("no video arg and no mp4 found in {ASSETS_DIR}/"))?,
    };

    if !video.is_file() {
        return Err(format!("video not found: {}", video.display()).into());
    }

    if name.is_empty() {
        return Err("session name must not be empty".into());
    }

    Ok(Args { video, name })
}

fn print_usage() {
    eprintln!(
        "Usage: cutlass-app [VIDEO] [--name NAME]\n\
         \n\
         Builds a one-clip project, saves to .cutlass/projects/, exports MP4 to .cutlass/exports/.\n\
         \n\
         VIDEO   Source file (default: first .mp4 in assets/)\n\
         --name  Session basename (default: demo, or CUTLASS_NAME env)\n"
    );
}

fn first_asset_in(dir: &str) -> Option<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "mp4"))
        .collect();
    paths.sort();
    paths.into_iter().next()
}

fn cutlass_paths(name: &str) -> Result<(PathBuf, PathBuf, PathBuf), AnyError> {
    let root = PathBuf::from(CUTLASS_DIR);
    fs::create_dir_all(root.join(PROJECTS_SUBDIR))?;
    fs::create_dir_all(root.join(EXPORTS_SUBDIR))?;
    fs::create_dir_all(root.join(CACHE_SUBDIR))?;
    let project = root.join(PROJECTS_SUBDIR).join(format!("{name}.cutlass"));
    let export = root.join(EXPORTS_SUBDIR).join(format!("{name}.mp4"));
    let cache = root.join(CACHE_SUBDIR);
    Ok((project, export, cache))
}

fn abs(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn run() -> Result<(), AnyError> {
    cutlass_engine::init();
    let args = parse_args()?;
    let (project_path, export_path, cache_dir) = cutlass_paths(&args.name)?;

    info!(
        video = %args.video.display(),
        name = %args.name,
        "starting e2e session"
    );

    let config = EngineConfig {
        cache_dir,
        ..EngineConfig::default()
    };

    let mut engine = Engine::new(config)?;

    let media_id = match engine.apply(Command::Project(ProjectCommand::Import {
        path: args.video.clone(),
    }))? {
        ApplyOutcome::Imported { media } => media,
        other => return Err(format!("import failed: {other:?}").into()),
    };

    let (clip_len, media_rate) = {
        let media = engine
            .project()
            .media(media_id)
            .ok_or("imported media missing from pool")?;
        (
            media.duration.value.clamp(1, MAX_EXPORT_FRAMES),
            media.frame_rate,
        )
    };

    let track_id = match engine.apply(Command::Edit(EditCommand::AddTrack {
        kind: TrackKind::Video,
        name: "V1".into(),
    }))? {
        ApplyOutcome::Edited(EditOutcome::CreatedTrack(id)) => id,
        other => return Err(format!("add track failed: {other:?}").into()),
    };

    let tl_rate = engine.project().timeline().frame_rate;
    let source = TimeRange::at_rate(0, clip_len, media_rate);
    let start = RationalTime::new(0, tl_rate);

    match engine.apply(Command::Edit(EditCommand::AddClip {
        track: track_id,
        media: media_id,
        source,
        start,
    }))? {
        ApplyOutcome::Edited(EditOutcome::Created(_)) => {}
        other => return Err(format!("add clip failed: {other:?}").into()),
    }

    let preview = engine
        .get_frame(RationalTime::new(0, tl_rate))
        .map_err(|e| format!("preview failed: {e}"))?;
    info!(
        width = preview.width,
        height = preview.height,
        bytes = preview.bytes.len(),
        "preview frame ok"
    );

    match engine.apply(Command::Project(ProjectCommand::Save {
        path: project_path.clone(),
    }))? {
        ApplyOutcome::Saved => {}
        other => return Err(format!("save failed: {other:?}").into()),
    };

    let export_start = Instant::now();
    let stats = match engine.apply(Command::Project(ProjectCommand::Export {
        path: export_path.clone(),
    }))? {
        ApplyOutcome::Exported { stats } => stats,
        other => return Err(format!("export failed: {other:?}").into()),
    };

    info!(
        frames = stats.frames,
        width = stats.width,
        height = stats.height,
        elapsed = ?export_start.elapsed(),
        project = %abs(&project_path).display(),
        export = %abs(&export_path).display(),
        "e2e session complete — open the MP4 path above in QuickTime/VLC"
    );

    Ok(())
}

fn main() {
    setup_tracing();
    if let Err(e) = run() {
        warn!(error = %e, "cutlass-app failed");
        std::process::exit(1);
    }
}
