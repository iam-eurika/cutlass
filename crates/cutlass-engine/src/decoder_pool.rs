//! Per-media decoder reuse for preview and export.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cutlass_decoder::{DecodeOptions, Decoder, HwAccel, KeyframeIndex};
use cutlass_models::MediaId;

use crate::error::EngineError;

struct Entry {
    path: PathBuf,
    decoder: Decoder,
    index: KeyframeIndex,
}

pub struct DecoderPool {
    entries: HashMap<MediaId, Entry>,
    options: DecodeOptions,
}

impl DecoderPool {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            options: DecodeOptions::default().hw_accel(HwAccel::None),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn decoder_and_index(
        &mut self,
        media_id: MediaId,
        path: &Path,
    ) -> Result<(&mut Decoder, &KeyframeIndex), EngineError> {
        let stale = self
            .entries
            .get(&media_id)
            .is_none_or(|e| e.path != path);

        if stale {
            let decoder = Decoder::open_with(path, self.options)?;
            let index = KeyframeIndex::build(path)?;
            self.entries.insert(
                media_id,
                Entry {
                    path: path.to_path_buf(),
                    decoder,
                    index,
                },
            );
        }

        let entry = self.entries.get_mut(&media_id).expect("just inserted");
        Ok((&mut entry.decoder, &entry.index))
    }
}

impl Default for DecoderPool {
    fn default() -> Self {
        Self::new()
    }
}
