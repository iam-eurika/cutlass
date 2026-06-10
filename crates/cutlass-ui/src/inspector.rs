//! Inspector helpers: resolve the selected clip and patch UI model fields.

use crate::{Clip, Project, SelectedClipInfo, Sequence, TrackKind};
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

pub fn set_text_content(project: &mut Project, track_id: &str, clip_id: &str, content: &str) {
    let sequence = project.sequence.clone();
    let tracks = sequence.tracks;

    for track_idx in 0..tracks.row_count() {
        let Some(track) = tracks.row_data(track_idx) else {
            continue;
        };
        if track.id != track_id {
            continue;
        }

        let clip_model = track.clips.clone();
        for clip_idx in 0..clip_model.row_count() {
            let Some(mut clip) = clip_model.row_data(clip_idx) else {
                continue;
            };
            if clip.id != clip_id {
                continue;
            }

            clip.text_content = content.into();
            clip_model.set_row_data(clip_idx, clip);
            tracks.set_row_data(track_idx, track);
            project.sequence = Sequence {
                tracks,
                ..sequence
            };
            return;
        }
    }
}
