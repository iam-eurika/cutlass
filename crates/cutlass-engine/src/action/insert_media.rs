use cutlass_models::MediaSource;

use super::{ApplyContext, EditAction};
use crate::error::EngineError;

pub struct InsertMediaAction {
    pub media: MediaSource,
}

impl EditAction for InsertMediaAction {
    fn apply(self: Box<Self>, ctx: &mut ApplyContext<'_>) -> Result<Box<dyn EditAction>, EngineError> {
        let id = self.media.id;
        ctx.project.add_media(self.media);
        Ok(Box::new(super::remove_media::RemoveMediaAction { media: id }))
    }
}
