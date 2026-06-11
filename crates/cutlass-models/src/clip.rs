use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::{ClipId, LinkId, MediaId};
use crate::time::{RationalTime, TimeRange, resample, time_add, time_sub};

/// What a clip draws. Either a trimmed range of imported media, or synthetic
/// content rendered by the engine (text, shapes, solids, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipSource {
    /// A trimmed portion of a [`MediaSource`](crate::MediaSource).
    ///
    /// `source` is the in/out within the media at the media's native rate.
    Media { media: MediaId, source: TimeRange },
    /// Engine-generated content with no backing file.
    Generated(Generator),
}

/// A synthetic clip with no source media. Parameters are intentionally minimal
/// for now; richer styling (fonts, transforms, gradients) can be added per
/// variant without touching the timeline model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Generator {
    /// A title / text layer.
    Text { content: String },
    /// A solid fill (RGBA, 0-255).
    SolidColor { rgba: [u8; 4] },
    /// A vector shape with a fill color (RGBA, 0-255). Geometry (a centered
    /// rect/ellipse) is fixed until per-layer transforms land.
    Shape {
        shape: Shape,
        /// Fill color. Old projects without this field default to white.
        #[serde(default = "default_shape_rgba")]
        rgba: [u8; 4],
    },
    /// Image or animated sticker (asset wiring TBD).
    Sticker,
    /// Motion / composited VFX layer (implementation TBD).
    Effect,
    /// Blur, mask, and similar pixel filters (implementation TBD).
    Filter,
    /// Color grade / pass-through layer affecting tracks beneath it.
    Adjustment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    Rectangle,
    Ellipse,
}

/// Default fill color for a shape without one (opaque white).
fn default_shape_rgba() -> [u8; 4] {
    [255, 255, 255, 255]
}

/// Spatial placement of a clip's content on the canvas (CapCut "Basic"
/// transform: position, scale, rotation, opacity).
///
/// Coordinates are normalized to the canvas so projects survive canvas-size
/// changes: `position` is the offset of the content center from the canvas
/// center as a fraction of canvas width/height (+x right, +y down — screen
/// convention). `scale` is uniform with 1.0 = aspect-fit inside the canvas
/// (CapCut's 100%). `rotation` is degrees clockwise about the content center.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClipTransform {
    /// Content-center offset from canvas center, normalized to canvas
    /// dimensions. `[0.0, 0.0]` = centered; `[0.5, 0.0]` = center sits on
    /// the right canvas edge.
    pub position: [f32; 2],
    /// Uniform scale; 1.0 aspect-fits the content inside the canvas.
    pub scale: f32,
    /// Clockwise rotation in degrees about the content center.
    pub rotation: f32,
    /// Layer opacity, 0.0 (transparent) ..= 1.0 (opaque).
    pub opacity: f32,
}

impl ClipTransform {
    pub const IDENTITY: Self = Self {
        position: [0.0, 0.0],
        scale: 1.0,
        rotation: 0.0,
        opacity: 1.0,
    };

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// `Ok` iff every component is finite, scale is positive, and opacity is
    /// within `0..=1` — the invariant [`crate::Project::set_transform`]
    /// enforces before storing.
    pub fn validate(&self) -> Result<(), ModelError> {
        let finite = self.position.iter().all(|v| v.is_finite())
            && self.scale.is_finite()
            && self.rotation.is_finite()
            && self.opacity.is_finite();
        if !finite {
            return Err(ModelError::InvalidTransform("non-finite component".into()));
        }
        if self.scale <= 0.0 {
            return Err(ModelError::InvalidTransform("scale must be positive".into()));
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err(ModelError::InvalidTransform("opacity must be in 0..=1".into()));
        }
        Ok(())
    }
}

impl Default for ClipTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A placement of some [`ClipSource`] on a track.
///
/// `timeline` is where the clip sits on the sequence, at the timeline rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub content: ClipSource,
    pub timeline: TimeRange,
    /// Link group (CapCut linkage): clips sharing a `LinkId` are selected,
    /// moved, and trimmed together — e.g. the video+audio pair created by
    /// dropping media with an audio stream. `None` ⇔ unlinked.
    #[serde(default)]
    pub link: Option<LinkId>,
    /// Spatial placement on the canvas. Identity (aspect-fit, centered) for
    /// clips created before transforms existed. Ignored on audio tracks.
    #[serde(default)]
    pub transform: ClipTransform,
}

impl Clip {
    /// A clip backed by a trimmed range of imported media.
    pub fn from_media(media: MediaId, source: TimeRange, timeline: TimeRange) -> Self {
        Self {
            id: ClipId::next(),
            content: ClipSource::Media { media, source },
            timeline,
            link: None,
            transform: ClipTransform::IDENTITY,
        }
    }

    /// A generated clip (text, shape, solid, ...).
    pub fn generated(generator: Generator, timeline: TimeRange) -> Self {
        Self {
            id: ClipId::next(),
            content: ClipSource::Generated(generator),
            timeline,
            link: None,
            transform: ClipTransform::IDENTITY,
        }
    }

    /// Timeline start position.
    pub fn start(&self) -> RationalTime {
        self.timeline.start
    }

    /// Exclusive timeline end.
    pub fn end(&self) -> Result<RationalTime, ModelError> {
        self.timeline.end()
    }

    /// The media this clip references, or `None` for generated content.
    pub fn media(&self) -> Option<MediaId> {
        match &self.content {
            ClipSource::Media { media, .. } => Some(*media),
            ClipSource::Generated(_) => None,
        }
    }

    /// The source in/out range, or `None` for generated content.
    pub fn source_range(&self) -> Option<TimeRange> {
        match &self.content {
            ClipSource::Media { source, .. } => Some(*source),
            ClipSource::Generated(_) => None,
        }
    }

    pub fn is_generated(&self) -> bool {
        matches!(self.content, ClipSource::Generated(_))
    }

    /// Map a timeline position to the corresponding source time, for media clips.
    pub fn source_time_at(&self, timeline_pos: RationalTime) -> Result<Option<RationalTime>, ModelError> {
        if !self.timeline.contains(timeline_pos)? {
            return Ok(None);
        }
        match &self.content {
            ClipSource::Media { source, .. } => {
                let offset_tl = time_sub(&timeline_pos, &self.timeline.start)?;
                let offset_src = resample(offset_tl, source.start.rate);
                Ok(Some(time_add(&source.start, &offset_src)?))
            }
            ClipSource::Generated(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Rational;

    const R24: Rational = Rational::FPS_24;
    const R30: Rational = Rational::FPS_30;

    fn rt(value: i64, rate: Rational) -> RationalTime {
        RationalTime::new(value, rate)
    }

    fn tr(start: i64, duration: i64, rate: Rational) -> TimeRange {
        TimeRange::at_rate(start, duration, rate)
    }

    fn media_clip(
        media: MediaId,
        source: TimeRange,
        timeline: TimeRange,
    ) -> Clip {
        Clip::from_media(media, source, timeline)
    }

    // --- constructors -----------------------------------------------------

    #[test]
    fn from_media_wires_content_and_timeline() {
        let media = MediaId::from_raw(42);
        let source = tr(100, 50, R30);
        let timeline = tr(10, 40, R24);
        let clip = media_clip(media, source, timeline);

        assert_eq!(
            clip.content,
            ClipSource::Media {
                media,
                source,
            }
        );
        assert_eq!(clip.timeline, timeline);
        assert!(!clip.is_generated());
    }

    #[test]
    fn from_media_assigns_distinct_ids() {
        let media = MediaId::from_raw(1);
        let source = tr(0, 10, R24);
        let timeline = tr(0, 10, R24);
        let a = media_clip(media, source, timeline);
        let b = media_clip(media, source, timeline);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn generated_text_clip() {
        let timeline = tr(0, 48, R24);
        let clip = Clip::generated(
            Generator::Text {
                content: "Hello".into(),
            },
            timeline,
        );
        assert_eq!(
            clip.content,
            ClipSource::Generated(Generator::Text {
                content: "Hello".into(),
            })
        );
        assert_eq!(clip.timeline, timeline);
        assert!(clip.is_generated());
    }

    #[test]
    fn generated_all_variants() {
        let timeline = tr(0, 10, R24);

        let solid = Clip::generated(
            Generator::SolidColor {
                rgba: [255, 0, 0, 255],
            },
            timeline,
        );
        assert!(matches!(
            solid.content,
            ClipSource::Generated(Generator::SolidColor { .. })
        ));

        let shape = Clip::generated(
            Generator::Shape {
                shape: Shape::Ellipse,
                rgba: [0, 128, 255, 255],
            },
            timeline,
        );
        assert!(matches!(
            shape.content,
            ClipSource::Generated(Generator::Shape {
                shape: Shape::Ellipse,
                ..
            })
        ));

        let adj = Clip::generated(Generator::Adjustment, timeline);
        assert!(matches!(
            adj.content,
            ClipSource::Generated(Generator::Adjustment)
        ));
    }

    #[test]
    fn generated_assigns_distinct_ids() {
        let timeline = tr(0, 10, R24);
        let a = Clip::generated(Generator::Adjustment, timeline);
        let b = Clip::generated(Generator::Adjustment, timeline);
        assert_ne!(a.id, b.id);
    }

    // --- accessors --------------------------------------------------------

    #[test]
    fn media_clip_accessors() {
        let media = MediaId::from_raw(7);
        let source = tr(50, 25, R24);
        let timeline = tr(100, 25, R24);
        let clip = media_clip(media, source, timeline);

        assert_eq!(clip.media(), Some(media));
        assert_eq!(clip.source_range(), Some(source));
        assert_eq!(clip.start(), rt(100, R24));
        assert_eq!(clip.end().unwrap(), rt(125, R24));
    }

    #[test]
    fn generated_clip_accessors_are_none() {
        let clip = Clip::generated(
            Generator::Text {
                content: "x".into(),
            },
            tr(5, 10, R24),
        );
        assert_eq!(clip.media(), None);
        assert_eq!(clip.source_range(), None);
        assert_eq!(clip.start().value, 5);
        assert_eq!(clip.end().unwrap().value, 15);
    }

    #[test]
    fn clip_clone_and_eq() {
        let media = MediaId::from_raw(1);
        let source = tr(0, 10, R24);
        let timeline = tr(0, 10, R24);
        let a = media_clip(media, source, timeline);
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.id, b.id);
    }

    // --- source_time_at: same-rate media ----------------------------------

    #[test]
    fn source_time_at_same_rate_maps_one_to_one() {
        // source [100, 110) placed at timeline [10, 20) — 1:1 at 24fps.
        let clip = media_clip(
            MediaId::from_raw(1),
            tr(100, 10, R24),
            tr(10, 10, R24),
        );

        assert_eq!(
            clip.source_time_at(rt(15, R24)).unwrap(),
            Some(rt(105, R24))
        );
        assert_eq!(
            clip.source_time_at(rt(10, R24)).unwrap(),
            Some(rt(100, R24))
        );
        assert_eq!(
            clip.source_time_at(rt(19, R24)).unwrap(),
            Some(rt(109, R24))
        );
    }

    #[test]
    fn source_time_at_half_open_boundaries() {
        let clip = media_clip(
            MediaId::from_raw(1),
            tr(0, 10, R24),
            tr(10, 10, R24),
        );

        // Exclusive end is not contained.
        assert_eq!(clip.source_time_at(rt(20, R24)).unwrap(), None);
        // Before start.
        assert_eq!(clip.source_time_at(rt(9, R24)).unwrap(), None);
        // After end.
        assert_eq!(clip.source_time_at(rt(21, R24)).unwrap(), None);
    }

    #[test]
    fn source_time_at_generated_always_none() {
        let clip = Clip::generated(
            Generator::Text {
                content: "title".into(),
            },
            tr(0, 100, R24),
        );
        assert_eq!(clip.source_time_at(rt(50, R24)).unwrap(), None);
    }

    // --- source_time_at: mixed rates ------------------------------------

    #[test]
    fn source_time_at_resamples_across_rates() {
        // 120 source ticks @ 30fps -> 96 timeline ticks @ 24fps.
        let clip = media_clip(
            MediaId::from_raw(1),
            tr(0, 120, R30),
            tr(0, 96, R24),
        );

        // Timeline midpoint should land near source midpoint after resample.
        let src = clip.source_time_at(rt(48, R24)).unwrap().unwrap();
        assert_eq!(src.rate, R30);
        // 48 @ 24fps = 60 @ 30fps offset from source start 0.
        assert_eq!(src.value, 60);

        // Timeline start maps to source start regardless of rate.
        assert_eq!(
            clip.source_time_at(rt(0, R24)).unwrap(),
            Some(rt(0, R30))
        );
    }

    #[test]
    fn source_time_at_offset_from_nonzero_source_start() {
        // source [200, 300) @ 30fps at timeline [0, 80) @ 24fps.
        let clip = media_clip(
            MediaId::from_raw(1),
            tr(200, 100, R30),
            tr(0, 80, R24),
        );

        let at_start = clip.source_time_at(rt(0, R24)).unwrap().unwrap();
        assert_eq!(at_start, rt(200, R30));

        // 40 timeline ticks @ 24fps -> 50 source ticks @ 30fps from in-point.
        let mid = clip.source_time_at(rt(40, R24)).unwrap().unwrap();
        assert_eq!(mid, rt(250, R30));
    }

    // --- transform ----------------------------------------------------------

    #[test]
    fn new_clips_have_identity_transform() {
        let clip = Clip::generated(Generator::Adjustment, tr(0, 10, R24));
        assert!(clip.transform.is_identity());
        assert_eq!(clip.transform, ClipTransform::default());
    }

    #[test]
    fn clip_without_transform_field_deserializes_to_identity() {
        // A clip serialized before transforms existed: no `transform` key.
        let clip = Clip::generated(
            Generator::Text {
                content: "old".into(),
            },
            tr(0, 10, R24),
        );
        let mut value = serde_json::to_value(&clip).expect("serialize");
        value
            .as_object_mut()
            .expect("clip serializes to a map")
            .remove("transform")
            .expect("transform field present");

        let loaded: Clip = serde_json::from_value(value).expect("deserialize legacy clip");
        assert!(loaded.transform.is_identity());
        assert_eq!(loaded.content, clip.content);
    }

    #[test]
    fn transform_roundtrips_through_serde() {
        let mut clip = Clip::generated(Generator::Adjustment, tr(0, 10, R24));
        clip.transform = ClipTransform {
            position: [-0.25, 0.5],
            scale: 1.5,
            rotation: 90.0,
            opacity: 0.25,
        };
        let json = serde_json::to_string(&clip).expect("serialize");
        let loaded: Clip = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(loaded.transform, clip.transform);
    }

    #[test]
    fn transform_validation() {
        assert!(ClipTransform::IDENTITY.validate().is_ok());
        assert!(
            ClipTransform {
                position: [0.4, -0.4],
                scale: 3.0,
                rotation: -720.0,
                opacity: 0.0,
            }
            .validate()
            .is_ok()
        );

        let bad_scale = ClipTransform {
            scale: -0.5,
            ..ClipTransform::IDENTITY
        };
        assert!(matches!(
            bad_scale.validate(),
            Err(ModelError::InvalidTransform(_))
        ));

        let bad_opacity = ClipTransform {
            opacity: -0.1,
            ..ClipTransform::IDENTITY
        };
        assert!(matches!(
            bad_opacity.validate(),
            Err(ModelError::InvalidTransform(_))
        ));

        let bad_position = ClipTransform {
            position: [0.0, f32::NAN],
            ..ClipTransform::IDENTITY
        };
        assert!(matches!(
            bad_position.validate(),
            Err(ModelError::InvalidTransform(_))
        ));
    }

    // --- source_time_at: errors -------------------------------------------

    #[test]
    fn source_time_at_rate_mismatch_errors() {
        let clip = media_clip(
            MediaId::from_raw(1),
            tr(0, 10, R24),
            tr(0, 10, R24),
        );
        let err = clip.source_time_at(rt(5, R30)).unwrap_err();
        assert_eq!(
            err,
            ModelError::RateMismatch {
                expected: R30,
                got: R24,
            }
        );
    }
}
