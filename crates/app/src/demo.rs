//! Dev/demo project seeded from `assets/` at the repo root.
//!
//! Resolved via `CUTLASS_ASSETS` or `CARGO_MANIFEST_DIR/../../assets`. When the
//! directory is missing (e.g. fresh clone without local fixtures), falls back
//! to an empty project so the editor still launches.

use std::path::{Path, PathBuf};

use models::{
    Clip, ClipId, Color, MediaId, MediaSource, Project, ProjectId, Rational,
    RationalTime, SchemaVersion, Sequence, SequenceId, TrackId, TrackKind,
};
use timeline::{AddClip, AddTrack, Command, apply};
use tracing::warn;

use crate::import;

const TIMEBASE: u32 = 90_000;
const DEFAULT_IMAGE_SECS: f64 = 5.0;
const DEMO_VIDEO_CLIP_SECS: f64 = 12.0;

/// Files placed on the timeline (must exist under [`assets_dir`]).
const TIMELINE_VIDEO: &str = "15881269_3840_2160_60fps_720p_proxy.mp4";
const TIMELINE_VOICE: &str = "ElevenLabs_2025-10-22T17_31_36_Brian_eleven_multilingual_v2.mp3";
const TIMELINE_MUSIC: &str = "baby.mp3";
const TIMELINE_IMAGE: &str = "texture.png";

const MEDIA_EXTENSIONS: &[&str] = &[
    "avi", "m4v", "mkv", "mov", "mp4", "mts", "ts", "webm", "wmv", "aac", "aif", "aiff", "flac",
    "m4a", "mp3", "ogg", "opus", "wav", "bmp", "gif", "jpeg", "jpg", "png", "tif", "tiff", "webp",
];

/// Build a project with probed media from `assets/` and a short starter edit.
pub fn project() -> Project {
    let assets = assets_dir();
    if !assets.is_dir() {
        warn!(
            ?assets,
            "demo: assets directory missing — open with empty project (set CUTLASS_ASSETS or add repo assets/)"
        );
        return crate::empty_project();
    }

    let mut project = base_project();
    let paths = collect_asset_paths(&assets);
    if paths.is_empty() {
        warn!(?assets, "demo: no media files in assets directory");
        return project;
    }

    for path in paths {
        project.media_bin.push(import::build_media_source(path));
    }

    if !populate_timeline(&mut project) {
        warn!("demo: starter clips skipped (missing timeline assets or overlap)");
    }

    project
}

fn base_project() -> Project {
    Project {
        id: ProjectId::new(),
        name: "Demo".into(),
        file_path: None,
        schema: SchemaVersion::CURRENT,
        sequence: Sequence {
            id: SequenceId::new(),
            name: "Demo Sequence".into(),
            width: 1920,
            height: 1080,
            fps: Rational::new_raw(30, 1),
            sample_rate: 48_000,
            timebase: TIMEBASE,
            duration: RationalTime::new_raw(0, TIMEBASE),
            in_point: None,
            out_point: None,
            tracks: Vec::new(),
        },
        media_bin: Vec::new(),
        is_dirty: false,
    }
}

fn populate_timeline(project: &mut Project) -> bool {
    let Some(video_id) = find_media(project, TIMELINE_VIDEO) else {
        warn!(file = TIMELINE_VIDEO, "demo: timeline asset missing");
        return false;
    };
    let Some(voice_id) = find_media(project, TIMELINE_VOICE) else {
        warn!(file = TIMELINE_VOICE, "demo: timeline asset missing");
        return false;
    };
    let Some(music_id) = find_media(project, TIMELINE_MUSIC) else {
        warn!(file = TIMELINE_MUSIC, "demo: timeline asset missing");
        return false;
    };
    let Some(image_id) = find_media(project, TIMELINE_IMAGE) else {
        warn!(file = TIMELINE_IMAGE, "demo: timeline asset missing");
        return false;
    };

    let v1 = TrackId::new();
    let v2 = TrackId::new();
    let a1 = TrackId::new();
    let a2 = TrackId::new();

    if apply(
        project,
        &Command::AddTrack(AddTrack {
            track_id: v1,
            kind: TrackKind::Video,
            name: "V1".into(),
            height_px: None,
        }),
    )
    .is_err()
    {
        return false;
    }
    if apply(
        project,
        &Command::AddTrack(AddTrack {
            track_id: v2,
            kind: TrackKind::Video,
            name: "V2".into(),
            height_px: None,
        }),
    )
    .is_err()
    {
        return false;
    }
    if apply(
        project,
        &Command::AddTrack(AddTrack {
            track_id: a1,
            kind: TrackKind::Audio,
            name: "A1 — Voice".into(),
            height_px: None,
        }),
    )
    .is_err()
    {
        return false;
    }
    if apply(
        project,
        &Command::AddTrack(AddTrack {
            track_id: a2,
            kind: TrackKind::Audio,
            name: "A2 — Music".into(),
            height_px: None,
        }),
    )
    .is_err()
    {
        return false;
    }

    let video = find_media_entry(project, video_id);
    let voice = find_media_entry(project, voice_id);
    let music = find_media_entry(project, music_id);
    let image = find_media_entry(project, image_id);

    let video_dur = clip_duration(video, Some(DEMO_VIDEO_CLIP_SECS));
    let voice_dur = clip_duration(voice, None);
    let music_dur = clip_duration(music, None);
    let image_dur = clip_duration(image, Some(DEFAULT_IMAGE_SECS));

    let t0 = rt(0);
    add_clip(
        project,
        v1,
        video_id,
        "B-roll",
        t0,
        video_dur,
        Color::rgb(64, 96, 200),
        1.0,
    )
    .is_ok()
        && add_clip(
            project,
            v2,
            image_id,
            "Texture",
            rt(ticks_from_secs(6.0)),
            image_dur,
            Color::rgb(180, 120, 64),
            1.0,
        )
        .is_ok()
        && add_clip(
            project,
            a1,
            voice_id,
            "Voiceover",
            t0,
            voice_dur,
            Color::rgb(96, 180, 120),
            1.0,
        )
        .is_ok()
        && add_clip(
            project,
            a2,
            music_id,
            "Music",
            t0,
            music_dur,
            Color::rgb(160, 96, 180),
            0.35,
        )
        .is_ok()
}

fn add_clip(
    project: &mut Project,
    track_id: TrackId,
    media_id: MediaId,
    name: &str,
    start: RationalTime,
    duration: RationalTime,
    color: Color,
    volume: f32,
) -> Result<(), timeline::TimelineError> {
    let clip_id = ClipId::new();
    apply(
        project,
        &Command::AddClip(AddClip {
            track_id,
            clip: Clip {
                id: clip_id,
                media_id: Some(media_id),
                track_id,
                name: name.into(),
                start,
                duration,
                source_in: rt(0),
                source_out: duration,
                speed: Rational::ONE,
                opacity: 1.0,
                volume,
                enabled: true,
                color,
            },
        }),
    )?;
    Ok(())
}

fn find_media(project: &Project, filename: &str) -> Option<MediaId> {
    project
        .media_bin
        .iter()
        .find(|m| m.path.file_name().is_some_and(|n| n == filename))
        .map(|m| m.id)
}

fn find_media_entry<'a>(project: &'a Project, id: MediaId) -> &'a MediaSource {
    project
        .media_bin
        .iter()
        .find(|m| m.id == id)
        .expect("media id was resolved from the same bin")
}

/// Clip length on the timeline; `cap_secs` limits long sources (e.g. 4K b-roll).
fn clip_duration(media: &MediaSource, cap_secs: Option<f64>) -> RationalTime {
    let media_ticks = to_timebase(media.duration, TIMEBASE);
    if media_ticks.num <= 0 {
        let fallback = cap_secs.unwrap_or(DEFAULT_IMAGE_SECS);
        return rt(ticks_from_secs(fallback));
    }
    match cap_secs {
        Some(cap) => rt(media_ticks.num.min(ticks_from_secs(cap))),
        None => media_ticks,
    }
}

fn collect_asset_paths(assets: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(assets)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_media_file(p))
        .collect();
    paths.sort_by_cached_key(|p| p.file_name().map(|n| n.to_owned()));
    paths
}

fn is_media_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| MEDIA_EXTENSIONS.iter().any(|&x| x.eq_ignore_ascii_case(ext)))
}

pub fn assets_dir() -> PathBuf {
    std::env::var_os("CUTLASS_ASSETS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")))
}

fn rt(num: i64) -> RationalTime {
    RationalTime::new_raw(num, TIMEBASE)
}

fn ticks_from_secs(secs: f64) -> i64 {
    (secs * f64::from(TIMEBASE)).round() as i64
}

fn to_timebase(t: RationalTime, tb: u32) -> RationalTime {
    if t.den == 0 {
        return RationalTime::ZERO;
    }
    if t.den == tb {
        return t;
    }
    let num = (i128::from(t.num) * i128::from(tb) / i128::from(t.den)) as i64;
    RationalTime::new_raw(num.max(0), tb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_dir_points_at_workspace_assets() {
        let dir = assets_dir();
        assert!(
            dir.ends_with("assets"),
            "expected path to end with assets/, got {}",
            dir.display()
        );
    }

    #[test]
    fn demo_project_builds_when_assets_present() {
        if !assets_dir().is_dir() {
            return;
        }
        let p = project();
        assert_eq!(p.name, "Demo");
        assert!(
            !p.media_bin.is_empty(),
            "expected probed media when assets/ exists"
        );
    }
}
