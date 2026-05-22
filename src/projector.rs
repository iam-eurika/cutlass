//! Slint view-model projection of the canonical Rust `Project`.
//!
//! The full domain → Slint conversion only runs **once**, at startup
//! (see [`Projector::new`]). After that the projector keeps id-keyed
//! handles to each track's clip `VecModel` plus an inner `clip_id → row`
//! map, so a single-clip mutation costs:
//!
//!   - 1 `HashMap` lookup by `&str` (no `String` allocation on the
//!     hot path — `HashMap<String, _>::get` accepts `&str` via `Borrow`),
//!   - 1 inner `HashMap` lookup by `&str`,
//!   - 1 `VecModel::row_data` clone of a `SlintClip` (refcounted
//!     `SharedString`s; no heap allocation in the steady state),
//!   - 1 `VecModel::set_row_data` write,
//!
//! and triggers a repaint of exactly that clip — no re-allocation of
//! any `VecModel`, no walk over unrelated tracks/clips. This is the
//! shape the command layer and the agent both feed mutations through.
//!
//! Structural mutations (add/remove track, add/remove clip, reorder)
//! will need to update the indices alongside the `VecModel`s. They
//! are intentionally not implemented yet — only `MoveClip` ships in
//! this first slice, which is a pure in-place edit.

use std::collections::HashMap;
use std::rc::Rc;

use slint::{Model, ModelRc, SharedString, VecModel};

use crate::models::{Clip, Project, Rational, RationalTime, TimeRange};
use crate::{
    Clip as SlintClip, Project as SlintProject, Rational as SlintRational,
    RationalTime as SlintRationalTime, Sequence as SlintSequence, TimeRange as SlintTimeRange,
    Track as SlintTrack,
};

/// Per-track Slint-side state needed for O(1) targeted updates.
struct TrackProjection {
    /// The `VecModel` backing this track's `clips` array in Slint.
    /// We keep a strong `Rc` so we can mutate rows in place after the
    /// projection has been handed to the UI (the `ModelRc` inside
    /// `SlintTrack.clips` shares the same allocation).
    clips: Rc<VecModel<SlintClip>>,
    /// `clip_id → row` in the `clips` model above. Built once during
    /// the initial projection; will need to be patched whenever a
    /// future command reorders / adds / removes clips on this track.
    clip_row: HashMap<String, usize>,
}

/// Owns the Slint-facing `Project` plus the indices needed to push
/// targeted updates into it without re-projecting from scratch.
pub struct Projector {
    project: SlintProject,
    tracks: HashMap<String, TrackProjection>,
}

impl Projector {
    /// Build the initial projection of `project` and the index tables
    /// that subsequent commands will use to patch it surgically.
    pub fn new(project: &Project) -> Self {
        let sequence = &project.sequence;

        let mut tracks_index: HashMap<String, TrackProjection> =
            HashMap::with_capacity(sequence.track_order.len());
        let mut slint_tracks: Vec<SlintTrack> = Vec::with_capacity(sequence.track_order.len());

        for track_id in &sequence.track_order {
            let Some(track) = sequence.tracks.get(track_id) else {
                continue;
            };

            let mut slint_clips: Vec<SlintClip> = Vec::with_capacity(track.clip_order.len());
            let mut clip_row: HashMap<String, usize> = HashMap::with_capacity(track.clip_order.len());

            for (row, clip_id) in track.clip_order.iter().enumerate() {
                let Some(clip) = track.clips.get(clip_id) else {
                    continue;
                };
                slint_clips.push(clip_to_slint(clip));
                clip_row.insert(clip_id.clone(), row);
            }

            let clips_model = Rc::new(VecModel::from(slint_clips));
            slint_tracks.push(SlintTrack {
                id: SharedString::from(track.id.as_str()),
                name: SharedString::from(track.name.as_str()),
                clips: ModelRc::from(clips_model.clone()),
            });
            tracks_index.insert(
                track.id.clone(),
                TrackProjection {
                    clips: clips_model,
                    clip_row,
                },
            );
        }

        let slint_project = SlintProject {
            id: SharedString::from(project.id.as_str()),
            title: SharedString::from(project.title.as_str()),
            sequence: SlintSequence {
                id: SharedString::from(sequence.id.as_str()),
                name: SharedString::from(sequence.name.as_str()),
                fps: rational_to_slint(&sequence.fps),
                drop_frame: sequence.drop_frame,
                tracks: ModelRc::new(Rc::new(VecModel::from(slint_tracks))),
                width: sequence.width,
                height: sequence.height,
            },
        };

        Self {
            project: slint_project,
            tracks: tracks_index,
        }
    }

    /// The Slint-facing `Project`. Cheap to `clone` — every field is
    /// either `Copy`, a refcounted `SharedString`, or a `ModelRc`
    /// (an `Rc` under the hood), so the clone the caller does to hand
    /// it to `EditorStore::set_project` is just a handful of refcount
    /// bumps.
    #[inline]
    pub fn slint_project(&self) -> &SlintProject {
        &self.project
    }

    /// Patch a single clip's `timeline_start.value` in the Slint
    /// projection. Returns `false` if the (track, clip) pair isn't
    /// indexed — caller decides whether that's an error or a no-op.
    ///
    /// Touches exactly one `VecModel` row; Slint repaints just that
    /// clip. The clip's rate is preserved (a move never changes
    /// rate), matching the contract of `Command::MoveClip`.
    pub fn move_clip(&self, track_id: &str, clip_id: &str, new_start_value: i32) -> bool {
        let Some(track) = self.tracks.get(track_id) else {
            return false;
        };
        let Some(&row) = track.clip_row.get(clip_id) else {
            return false;
        };
        let Some(mut clip) = track.clips.row_data(row) else {
            return false;
        };
        clip.timeline_start.value = new_start_value;
        track.clips.set_row_data(row, clip);
        true
    }
}

#[inline]
fn clip_to_slint(clip: &Clip) -> SlintClip {
    SlintClip {
        id: SharedString::from(clip.id.as_str()),
        name: SharedString::from(clip.name.as_str()),
        timeline_start: rational_time_to_slint(&clip.timeline_start),
        source_range: time_range_to_slint(&clip.source_range),
    }
}

#[inline]
fn rational_to_slint(r: &Rational) -> SlintRational {
    SlintRational {
        num: r.num,
        den: r.den,
    }
}

#[inline]
fn rational_time_to_slint(rt: &RationalTime) -> SlintRationalTime {
    SlintRationalTime {
        value: rt.value,
        rate: rational_to_slint(&rt.rate),
    }
}

#[inline]
fn time_range_to_slint(range: &TimeRange) -> SlintTimeRange {
    SlintTimeRange {
        start: rational_time_to_slint(&range.start),
        duration: rational_time_to_slint(&range.duration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::sample_project;

    #[test]
    fn projection_preserves_track_and_clip_order() {
        let domain = sample_project();
        let projector = Projector::new(&domain);
        let slint = projector.slint_project();

        assert_eq!(
            slint.sequence.tracks.row_count(),
            domain.sequence.track_order.len()
        );

        for (i, track_id) in domain.sequence.track_order.iter().enumerate() {
            let track = domain.sequence.tracks.get(track_id).unwrap();
            let slint_track = slint.sequence.tracks.row_data(i).unwrap();
            assert_eq!(slint_track.id, track.id);
            assert_eq!(slint_track.clips.row_count(), track.clip_order.len());

            for (j, clip_id) in track.clip_order.iter().enumerate() {
                let clip = track.clips.get(clip_id).unwrap();
                let slint_clip = slint_track.clips.row_data(j).unwrap();
                assert_eq!(slint_clip.id, clip.id);
                assert_eq!(slint_clip.timeline_start.value, clip.timeline_start.value);
            }
        }
    }

    #[test]
    fn move_clip_patches_only_the_targeted_row() {
        let domain = sample_project();
        let projector = Projector::new(&domain);

        // Capture the state of every other clip so we can assert
        // nothing else moved. (Defends against a future bug that
        // accidentally rewrites the whole row, or worse, the whole
        // track model.)
        let before: Vec<(usize, usize, SlintClip)> = {
            let slint = projector.slint_project();
            let mut out = Vec::new();
            for ti in 0..slint.sequence.tracks.row_count() {
                let t = slint.sequence.tracks.row_data(ti).unwrap();
                for ci in 0..t.clips.row_count() {
                    out.push((ti, ci, t.clips.row_data(ci).unwrap()));
                }
            }
            out
        };

        // Move "Clip 1" on "Track 1" from 10 → 42.
        assert!(projector.move_clip("1", "1", 42));

        let slint = projector.slint_project();
        for (ti, ci, prev) in before {
            let now = slint
                .sequence
                .tracks
                .row_data(ti)
                .unwrap()
                .clips
                .row_data(ci)
                .unwrap();
            if prev.id == "1" {
                assert_eq!(now.timeline_start.value, 42, "target clip not updated");
                // Rate and source_range must be preserved on move.
                assert_eq!(now.timeline_start.rate.num, prev.timeline_start.rate.num);
                assert_eq!(now.timeline_start.rate.den, prev.timeline_start.rate.den);
                assert_eq!(
                    now.source_range.duration.value,
                    prev.source_range.duration.value
                );
            } else {
                assert_eq!(
                    now.timeline_start.value, prev.timeline_start.value,
                    "unrelated clip {:?} was modified",
                    prev.id
                );
            }
        }
    }

    #[test]
    fn move_clip_returns_false_for_unknown_ids() {
        let domain = sample_project();
        let projector = Projector::new(&domain);

        assert!(!projector.move_clip("does-not-exist", "1", 0));
        assert!(!projector.move_clip("1", "does-not-exist", 0));
    }
}
