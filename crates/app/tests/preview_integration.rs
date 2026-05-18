//! Preview pipeline integration tests (GPU + FFmpeg required).

#![cfg(unix)]

use decoder::Rational;
use app::{
    fixture_available, h264_fixture_path, pixel_variance_rgba, plan_playhead, rgba_fingerprint,
    PreviewOutcome, PreviewSession, PREVIEW_MAX_EDGE_PX,
};

fn require_fixture() -> std::path::PathBuf {
    let path = h264_fixture_path();
    if !fixture_available(&path) {
        eprintln!(
            "skip: fixture missing at {} (run crates/decoder/tests/assets/regenerate.sh)",
            path.display()
        );
        std::process::exit(0);
    }
    path
}

#[test]
fn playhead_plan_matches_timeline_inside_clip() {
    let path = require_fixture();
    let (project, track, _, _) = PreviewSession::single_clip_project(&path).expect("project");
    let plan = plan_playhead(&project, track, Rational::new_raw(2, 1))
        .expect("plan")
        .expect("inside clip");
    assert_eq!(plan.media_time.reduced(), Rational::new_raw(2, 1));
}

#[test]
fn playhead_plan_gap_beyond_clip() {
    let path = require_fixture();
    let (project, track, _, _) = PreviewSession::single_clip_project(&path).expect("project");
    assert!(
        plan_playhead(&project, track, Rational::new_raw(120, 1))
            .expect("plan")
            .is_none()
    );
}

#[test]
fn preview_at_zero_yields_non_blank_frame() {
    let path = require_fixture();
    let (project, track, _, _) = PreviewSession::single_clip_project(&path).expect("project");
    let mut session = PreviewSession::from_project(project, track).expect("GPU session");

    let outcome = session
        .preview_at(Rational::new_raw(0, 1))
        .expect("preview");
    let PreviewOutcome::Frame { rgba, width, height, .. } = outcome else {
        panic!("expected frame at t=0");
    };
    assert!(width > 0 && height > 0);
    assert!(width.max(height) <= PREVIEW_MAX_EDGE_PX);
    assert_eq!(rgba.len(), (width * height * 4) as usize);
    assert!(
        pixel_variance_rgba(&rgba) > 5.0,
        "expected non-flat RGBA (blank frame)"
    );
}

#[test]
fn preview_gap_when_no_clip() {
    let path = require_fixture();
    let (project, track, _, _) = PreviewSession::single_clip_project(&path).expect("project");
    let mut session = PreviewSession::from_project(project, track).expect("GPU session");

    let outcome = session
        .preview_at(Rational::new_raw(500, 1))
        .expect("preview");
    assert!(matches!(outcome, PreviewOutcome::Gap));
}

#[test]
fn preview_different_times_differ_or_both_valid() {
    let path = require_fixture();
    let (project, track, _, _) = PreviewSession::single_clip_project(&path).expect("project");
    let mut session = PreviewSession::from_project(project, track).expect("GPU session");

    let a = session
        .preview_at(Rational::new_raw(0, 1))
        .expect("t=0");
    let b = session
        .preview_at(Rational::new_raw(2, 1))
        .expect("t=2");
    let fa = match a {
        PreviewOutcome::Frame { rgba, .. } => rgba_fingerprint(&rgba),
        _ => panic!("t=0 frame"),
    };
    let fb = match b {
        PreviewOutcome::Frame { rgba, .. } => rgba_fingerprint(&rgba),
        _ => panic!("t=2 frame"),
    };
    assert_ne!(fa, fb, "seek to 2s should differ from t=0");
}

#[test]
fn set_source_probed_after_open() {
    let path = require_fixture();
    let (project, track, _, _) = PreviewSession::single_clip_project(&path).expect("project");
    let source_id = *project.sources.keys().next().unwrap();
    let mut session = PreviewSession::from_project(project, track).expect("GPU session");

    session.ensure_source_open(source_id).expect("open");
    let probed = session
        .project
        .sources
        .get(&source_id)
        .and_then(|s| s.probed.as_ref())
        .expect("probed metadata");
    assert!(probed.width > 0);
    assert!(probed.height > 0);
}
