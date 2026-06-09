use cutlass_models::MediaId;

use super::{ApplyContext, EditAction};
use crate::error::EngineError;

pub struct RemoveMediaAction {
    pub media: MediaId,
}

impl EditAction for RemoveMediaAction {
    fn apply(self: Box<Self>, ctx: &mut ApplyContext<'_>) -> Result<Box<dyn EditAction>, EngineError> {
        let media = ctx.project.remove_media(self.media)?;
        Ok(Box::new(super::insert_media::InsertMediaAction { media }))
    }
}
