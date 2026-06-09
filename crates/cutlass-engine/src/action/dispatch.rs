use cutlass_commands::{Command, EditCommand, EditOutcome, ProjectCommand};

use super::add_clip;
use super::import;
use super::legacy::apply_edit_legacy;
use super::remove_clip::RemoveClipAction;
use super::{ApplyContext, EditAction};
use crate::error::EngineError;

/// Result of applying a wire [`Command`] through the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Imported { media: cutlass_models::MediaId },
    Saved,
    Opened,
    Loaded,
    Edited(EditOutcome),
}

pub fn dispatch(
    command: Command,
    ctx: &mut ApplyContext<'_>,
) -> Result<(ApplyOutcome, Option<Box<dyn EditAction>>), EngineError> {
    match command {
        Command::Project(project) => dispatch_project(project, ctx),
        Command::Edit(edit) => dispatch_edit(edit, ctx),
    }
}

fn dispatch_project(
    command: ProjectCommand,
    ctx: &mut ApplyContext<'_>,
) -> Result<(ApplyOutcome, Option<Box<dyn EditAction>>), EngineError> {
    match command {
        ProjectCommand::Import { path } => {
            let (media, inverse) = import::execute(ctx, &path)?;
            Ok((ApplyOutcome::Imported { media }, Some(inverse)))
        }
        ProjectCommand::Save { path } => {
            crate::session::save_project(ctx.project, &path)?;
            *ctx.project_path = Some(path);
            Ok((ApplyOutcome::Saved, None))
        }
        ProjectCommand::Open { path } => {
            let loaded = crate::session::load_project(&path)?;
            crate::session::relink_media_cache(ctx.cache, &loaded, true)?;
            crate::session::replace_session(ctx.project, &mut ctx.project_path, loaded, path);
            ctx.history.clear();
            Ok((ApplyOutcome::Opened, None))
        }
        ProjectCommand::Load { path } => {
            let loaded = crate::session::load_project(&path)?;
            crate::session::relink_media_cache(ctx.cache, &loaded, false)?;
            crate::session::replace_session(ctx.project, &mut ctx.project_path, loaded, path);
            ctx.history.clear();
            Ok((ApplyOutcome::Loaded, None))
        }
    }
}

fn dispatch_edit(
    edit: EditCommand,
    ctx: &mut ApplyContext<'_>,
) -> Result<(ApplyOutcome, Option<Box<dyn EditAction>>), EngineError> {
    match edit {
        EditCommand::AddClip {
            track,
            media,
            source,
            start,
        } => {
            let (id, inverse) = add_clip::execute(ctx, track, media, source, start)?;
            Ok((ApplyOutcome::Edited(EditOutcome::Created(id)), Some(inverse)))
        }
        EditCommand::RemoveClip { clip } => {
            let inverse = Box::new(RemoveClipAction { clip }).apply(ctx)?;
            Ok((ApplyOutcome::Edited(EditOutcome::Removed(clip)), Some(inverse)))
        }
        other => apply_edit_legacy(ctx, other)
            .map(|(outcome, inverse)| (ApplyOutcome::Edited(outcome), Some(inverse))),
    }
}
