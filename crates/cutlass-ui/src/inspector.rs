//! Inspector helpers: resolve the selected clip for the property sheet.

use crate::{Clip, SelectedClipInfo, Sequence, TrackKind};
use slint::Model;

pub fn resolve_selection(
    sequence: Sequence,
    track_id: &str,
    clip_id: &str,
) -> SelectedClipInfo {
    if track_id.is_empty() || clip_id.is_empty() {
        return SelectedClipInfo {
            found: false,
            track_kind: TrackKind::Video,
            clip: Clip::default(),
        };
    }

    for track_idx in 0..sequence.tracks.row_count() {
        let Some(track) = sequence.tracks.row_data(track_idx) else {
            continue;
        };
        if track.id != track_id {
            continue;
        }

        for clip_idx in 0..track.clips.row_count() {
            let Some(clip) = track.clips.row_data(clip_idx) else {
                continue;
            };
            if clip.id == clip_id {
                return SelectedClipInfo {
                    found: true,
                    track_kind: track.kind,
                    clip,
                };
            }
        }
    }

    SelectedClipInfo {
        found: false,
        track_kind: TrackKind::Video,
        clip: Clip::default(),
    }
}
