//! Plain Rust mirrors of the Slint project models (`ui/models/project.slint`).
//!
//! Use these at engine / persistence / agent boundaries so logic does not
//! depend on Slint's `SharedString`, `ModelRc`, or `Coord` types.

use crate::{Clip, Project, Rational, RationalTime, Sequence, TimeRange, Track};
use slint::{Model, ModelRc, SharedString, VecModel};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RationalDto {
    pub num: i32,
    pub den: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RationalTimeDto {
    pub value: i32,
    pub rate: RationalDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeRangeDto {
    pub start: RationalTimeDto,
    pub duration: RationalTimeDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipDto {
    pub id: String,
    pub name: String,
    pub timeline_start: RationalTimeDto,
    pub source_range: TimeRangeDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackDto {
    pub id: String,
    pub name: String,
    pub clips: Vec<ClipDto>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SequenceDto {
    pub id: String,
    pub name: String,
    pub fps: RationalDto,
    pub drop_frame: bool,
    pub tracks: Vec<TrackDto>,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectDto {
    pub id: String,
    pub title: String,
    pub sequence: SequenceDto,
}

fn model_to_vec<T: Clone>(model: &ModelRc<T>) -> Vec<T> {
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .collect()
}

impl From<Rational> for RationalDto {
    fn from(value: Rational) -> Self {
        Self {
            num: value.num,
            den: value.den,
        }
    }
}

impl From<RationalDto> for Rational {
    fn from(value: RationalDto) -> Self {
        Self {
            num: value.num,
            den: value.den,
        }
    }
}

impl From<RationalTime> for RationalTimeDto {
    fn from(value: RationalTime) -> Self {
        Self {
            value: value.value,
            rate: value.rate.into(),
        }
    }
}

impl From<RationalTimeDto> for RationalTime {
    fn from(value: RationalTimeDto) -> Self {
        Self {
            value: value.value,
            rate: value.rate.into(),
        }
    }
}

impl From<TimeRange> for TimeRangeDto {
    fn from(value: TimeRange) -> Self {
        Self {
            start: value.start.into(),
            duration: value.duration.into(),
        }
    }
}

impl From<TimeRangeDto> for TimeRange {
    fn from(value: TimeRangeDto) -> Self {
        Self {
            start: value.start.into(),
            duration: value.duration.into(),
        }
    }
}

impl From<Clip> for ClipDto {
    fn from(value: Clip) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.to_string(),
            timeline_start: value.timeline_start.into(),
            source_range: value.source_range.into(),
        }
    }
}

impl From<ClipDto> for Clip {
    fn from(value: ClipDto) -> Self {
        Self {
            id: SharedString::from(value.id),
            name: SharedString::from(value.name),
            timeline_start: value.timeline_start.into(),
            source_range: value.source_range.into(),
        }
    }
}

impl From<Track> for TrackDto {
    fn from(value: Track) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.to_string(),
            clips: model_to_vec(&value.clips)
                .into_iter()
                .map(ClipDto::from)
                .collect(),
        }
    }
}

impl From<TrackDto> for Track {
    fn from(value: TrackDto) -> Self {
        Self {
            id: SharedString::from(value.id),
            name: SharedString::from(value.name),
            clips: ModelRc::new(VecModel::from(
                value
                    .clips
                    .into_iter()
                    .map(Clip::from)
                    .collect::<Vec<_>>(),
            )),
        }
    }
}

impl From<Sequence> for SequenceDto {
    fn from(value: Sequence) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.to_string(),
            fps: value.fps.into(),
            drop_frame: value.drop_frame,
            tracks: model_to_vec(&value.tracks)
                .into_iter()
                .map(TrackDto::from)
                .collect(),
            width: value.width,
            height: value.height,
        }
    }
}

impl From<SequenceDto> for Sequence {
    fn from(value: SequenceDto) -> Self {
        Self {
            id: SharedString::from(value.id),
            name: SharedString::from(value.name),
            fps: value.fps.into(),
            drop_frame: value.drop_frame,
            tracks: ModelRc::new(VecModel::from(
                value
                    .tracks
                    .into_iter()
                    .map(Track::from)
                    .collect::<Vec<_>>(),
            )),
            width: value.width,
            height: value.height,
        }
    }
}

impl From<Project> for ProjectDto {
    fn from(value: Project) -> Self {
        Self {
            id: value.id.to_string(),
            title: value.title.to_string(),
            sequence: value.sequence.into(),
        }
    }
}

impl From<ProjectDto> for Project {
    fn from(value: ProjectDto) -> Self {
        Self {
            id: SharedString::from(value.id),
            title: SharedString::from(value.title),
            sequence: value.sequence.into(),
        }
    }
}

/// Rebuild a Slint project with [`VecModel`]-backed track/clip lists so
/// Rust can mutate via [`Model::set_row_data`]. Inline literal arrays in
/// `.slint` files compile to read-only [`MapModel`] adapters.
pub fn hydrate_project(project: Project) -> Project {
    ProjectDto::from(project).into()
}
