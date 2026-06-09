use cutlass_models::{Clip, TrackId};

use super::{ApplyContext, EditAction};
use crate::error::EngineError;

pub struct InsertClipAction {
    pub track: TrackId,
    pub clip: Clip,
}

impl EditAction for InsertClipAction {
    fn apply(self: Box<Self>, ctx: &mut ApplyContext<'_>) -> Result<Box<dyn EditAction>, EngineError> {
        let id = self.clip.id;
        ctx.project.timeline_mut().add_clip(self.track, self.clip)?;
        Ok(Box::new(super::remove_clip::RemoveClipAction { clip: id }))
    }
}
