//! Project session operations: save, open, load.

use std::path::{Path, PathBuf};

use cutlass_cache::{CacheSpec, FrameCache, SourceFingerprint};
use cutlass_models::Project;

use crate::error::EngineError;

/// Relink on-disk cache entries for media that exists on disk.
pub fn relink_media_cache(
    cache: &FrameCache,
    project: &Project,
    strict: bool,
) -> Result<(), EngineError> {
    for media in project.media_iter() {
        if !media.path().exists() {
            if strict {
                return Err(EngineError::MissingMedia(
                    media.path().display().to_string(),
                ));
            }
            continue;
        }
        let fingerprint = SourceFingerprint::from_path(media.path())?;
        let spec = CacheSpec {
            width: media.width,
            height: media.height,
            pixfmt: "yuv420p".into(),
        };
        cache
            .register_source(fingerprint, spec)
            .map_err(EngineError::from)?;
    }
    Ok(())
}

pub fn save_project(project: &Project, path: &Path) -> Result<(), EngineError> {
    project.save_to_file(path)?;
    Ok(())
}

pub fn load_project(path: &Path) -> Result<Project, EngineError> {
    Ok(Project::load_from_file(path)?)
}

pub fn replace_session(
    project: &mut Project,
    project_path: &mut Option<PathBuf>,
    loaded: Project,
    path: PathBuf,
) {
    *project = loaded;
    *project_path = Some(path);
}
