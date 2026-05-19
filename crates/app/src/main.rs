slint::include_modules!();

use slint::BackendSelector;
use slint::wgpu_28::WGPUConfiguration;
use slint::{ModelRc, VecModel};
use std::rc::Rc;
use tracing_subscriber::EnvFilter;

const TIMEBASE: i32 = 90_000;

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    BackendSelector::new()
        .require_wgpu_28(WGPUConfiguration::default())
        .select()?;

    let mut app = AppWindow::new()?;
    create_project(&mut app);
    app.run()?;
    Ok(())
}

fn rt(num: i32) -> UiRationalTime {
    UiRationalTime {
        num,
        den: TIMEBASE,
    }
}

fn create_project(app: &mut AppWindow) {
    let media_intro = "media-intro";
    let media_broll = "media-broll";
    let media_music = "media-music";
    let seq_main = "seq-main";
    let track_v1 = "track-v1";
    let track_a1 = "track-a1";

    let media_bin: Vec<UiMediaSource> = vec![
        UiMediaSource {
            id: media_intro.into(),
            name: "intro.mp4".into(),
            path: "/Users/demo/Videos/intro.mp4".into(),
            kind: UiMediaKind::Video,
            has_video: true,
            has_audio: true,
            duration: rt(900_000),
            width: 1920,
            height: 1080,
            fps_num: 30_000,
            fps_den: 1_001,
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            is_supported: true,
            thumbnail: Default::default(),
            is_loading: false,
            is_missing: false,
            error: "".into(),
        },
        UiMediaSource {
            id: media_broll.into(),
            name: "broll.mp4".into(),
            path: "/Users/demo/Videos/broll.mp4".into(),
            kind: UiMediaKind::Video,
            has_video: true,
            has_audio: false,
            duration: rt(1_350_000),
            width: 3840,
            height: 2160,
            fps_num: 24,
            fps_den: 1,
            video_codec: "hevc".into(),
            audio_codec: "".into(),
            is_supported: true,
            thumbnail: Default::default(),
            is_loading: false,
            is_missing: false,
            error: "".into(),
        },
        UiMediaSource {
            id: media_music.into(),
            name: "background-music.wav".into(),
            path: "/Users/demo/Audio/background-music.wav".into(),
            kind: UiMediaKind::Audio,
            has_video: false,
            has_audio: true,
            duration: rt(2_700_000),
            width: 0,
            height: 0,
            fps_num: 0,
            fps_den: 1,
            video_codec: "".into(),
            audio_codec: "pcm_s16le".into(),
            is_supported: true,
            thumbnail: Default::default(),
            is_loading: false,
            is_missing: false,
            error: "".into(),
        },
    ];

    let tracks: Vec<UiTrack> = vec![
        UiTrack {
            id: track_v1.into(),
            name: "V1".into(),
            kind: UiTrackKind::Video,
            height_px: 72,
            muted: false,
            solo: false,
            locked: false,
            visible: true,
            clips: model(vec![
                UiClip {
                    id: "clip-intro".into(),
                    media_id: media_intro.into(),
                    track_id: track_v1.into(),
                    name: "Intro".into(),
                    start: rt(0),
                    duration: rt(270_000),
                    source_in: rt(0),
                    source_out: rt(270_000),
                    speed: rt(TIMEBASE),
                    opacity: 1.0,
                    volume: 1.0,
                    enabled: true,
                    selected: false,
                    color: slint::Color::from_rgb_u8(70, 130, 180),
                },
                UiClip {
                    id: "clip-broll".into(),
                    media_id: media_broll.into(),
                    track_id: track_v1.into(),
                    name: "B-Roll".into(),
                    start: rt(270_000),
                    duration: rt(450_000),
                    source_in: rt(90_000),
                    source_out: rt(540_000),
                    speed: rt(TIMEBASE),
                    opacity: 1.0,
                    volume: 1.0,
                    enabled: true,
                    selected: true,
                    color: slint::Color::from_rgb_u8(100, 100, 157),
                },
            ]),
        },
        UiTrack {
            id: track_a1.into(),
            name: "A1".into(),
            kind: UiTrackKind::Audio,
            height_px: 48,
            muted: false,
            solo: false,
            locked: false,
            visible: true,
            clips: model(vec![UiClip {
                id: "clip-music".into(),
                media_id: media_music.into(),
                track_id: track_a1.into(),
                name: "Music".into(),
                start: rt(0),
                duration: rt(720_000),
                source_in: rt(0),
                source_out: rt(720_000),
                speed: rt(TIMEBASE),
                opacity: 1.0,
                volume: 0.8,
                enabled: true,
                selected: false,
                color: slint::Color::from_rgb_u8(60, 120, 90),
            }]),
        },
    ];

    let sequence = UiSequence {
        id: seq_main.into(),
        name: "Main Sequence".into(),
        width: 1920,
        height: 1080,
        fps_num: 30_000,
        fps_den: 1_001,
        sample_rate: 48_000,
        timebase: TIMEBASE,
        duration: rt(720_000),
        playhead: rt(135_000),
        has_in_point: true,
        in_point: rt(0),
        has_out_point: true,
        out_point: rt(720_000),
        zoom: 50.0,
        scroll: rt(0),
        tracks: model(tracks),
    };

    let project = UiProject {
        id: "project-demo".into(),
        name: "Demo Project".into(),
        file_path: "/Users/demo/Projects/demo.cutlass".into(),
        schema: UiSchemaVersion {
            major: 0,
            minor: 1,
            patch: 0,
        },
        sequence,
        media_bin: model(media_bin),
        is_dirty: false,
    };

    app.global::<AppState>().set_project(project);
}
