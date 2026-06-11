use cutlass_models::{ClipId, ClipTransform, ModelError};

use crate::action::edit::restore_clip::RestoreClipAction;
use crate::action::{ApplyContext, EditAction};
use crate::error::EngineError;

/// Set a clip's spatial transform (position/scale/rotation/opacity). The
/// inverse is a full-clip restore, so it oscillates like trim's
/// `RestoreClipAction`.
pub struct SetTransformAction {
    pub clip: ClipId,
    pub transform: ClipTransform,
}

pub fn execute(
    ctx: &mut ApplyContext<'_>,
    clip: ClipId,
    transform: ClipTransform,
) -> Result<Box<dyn EditAction>, EngineError> {
    let before = ctx
        .project
        .clip(clip)
        .cloned()
        .ok_or(ModelError::UnknownClip(clip))?;
    ctx.project.set_transform(clip, transform)?;
    Ok(Box::new(RestoreClipAction { clip: before }))
}

impl EditAction for SetTransformAction {
    fn apply(self: Box<Self>, ctx: &mut ApplyContext<'_>) -> Result<Box<dyn EditAction>, EngineError> {
        execute(ctx, self.clip, self.transform)
    }
}
