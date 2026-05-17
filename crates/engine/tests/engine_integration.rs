//! Integration tests: worker thread + FFmpeg against fixtures under `tests/assets/`.
//! Regenerate media with `tests/assets/regenerate.sh`.

#![cfg(unix)]

// Integration-test crate root is this file; submodule lives under `tests/engine_integration/`.
#[path = "engine_integration/common.rs"]
mod common;

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use common::{
    asset, drain_available, expect_eof_with_req, expect_frame_with_req, open_and_handshake,
    recv_opened,
};
use decoder::{DecoderError, PixelFormat, Rational};
use engine::{Engine, EngineError, EngineEvent, EventReceiver, SourceId};

// --- Open / probe ---

#[test]
fn open_emits_probe_metadata_for_h264_fixture() {
    let (engine, rx) = Engine::new();
    let (sid, rid) = engine.open(asset("testsrc_h264.mp4"));
    match rx.recv_timeout(Duration::from_secs(5)).expect("Opened") {
        EngineEvent::Opened {
            source_id,
            info,
            request_id,
        } => {
            assert_eq!(source_id, sid);
            assert_eq!(request_id, rid);
            assert_eq!(info.width, 320);
            assert_eq!(info.height, 240);
            assert_eq!(info.pixel_format, PixelFormat::Yuv420p);
            assert!(info.timebase.den > 0);
            let d = info.duration.expect("fixture duration");
            assert!(
                d.ge(Rational::new_raw(4, 1)) && !d.ge(Rational::new_raw(6, 1)),
                "duration ~5s, got {d}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn open_missing_path_returns_decoder_open_error_with_request_id() {
    let (engine, rx) = Engine::new();
    let bad = PathBuf::from("/no/such/cutlass_engine_missing.mp4");
    let (_sid, rid) = engine.open(bad);
    match rx.recv_timeout(Duration::from_secs(5)).expect("event") {
        EngineEvent::Error {
            source_id: Some(_),
            error: EngineError::Decoder(DecoderError::Open(_)),
            request_id: Some(r),
        } => assert_eq!(r, rid),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn open_audio_only_emits_unsupported_no_video() {
    let (engine, rx) = Engine::new();
    let (_sid, _rid) = engine.open(asset("audio_only.m4a"));
    match rx.recv_timeout(Duration::from_secs(5)).expect("event") {
        EngineEvent::Error {
            error: EngineError::Decoder(DecoderError::Unsupported { what }),
            ..
        } => assert!(
            what.contains("no video stream"),
            "unexpected message: {what}"
        ),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn open_rejects_unsupported_pixel_format_fixture() {
    let (engine, rx) = Engine::new();
    let (_sid, _rid) = engine.open(asset("test_unsupported_codec.mkv"));
    match rx.recv_timeout(Duration::from_secs(5)).expect("event") {
        EngineEvent::Error {
            error: EngineError::Decoder(DecoderError::Unsupported { what }),
            ..
        } => assert!(
            what.contains("pixel format") || what.contains("YUV420P") || what.contains("not"),
            "unexpected: {what}"
        ),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn open_corrupt_file_emits_decoder_open_error() {
    let (engine, rx) = Engine::new();
    let (_sid, _rid) = engine.open(asset("corrupt_truncated.mp4"));
    let res = rx.recv_timeout(Duration::from_secs(5)).expect("event");
    assert!(
        matches!(&res, EngineEvent::Error {
            error: EngineError::Decoder(DecoderError::Open(_)),
            ..
        }),
        "expected Open demuxer error, got {res:?}"
    );
}

#[test]
fn open_mixed_av_mp4_decodes_video_frames() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "test_av.mp4");

    let mut next_req = engine.next_frame(sid);
    for _ in 0..5 {
        match rx.recv_timeout(Duration::from_secs(5)).expect("decode") {
            EngineEvent::Frame {
                frame,
                request_id: Some(r),
                ..
            } => {
                assert_eq!(r, next_req);
                assert_eq!(frame.width, 128);
                assert_eq!(frame.height, 96);
                next_req = engine.next_frame(sid);
            }
            EngineEvent::Eof { .. } => panic!("eof before 5 video frames"),
            other => panic!("unexpected {other:?}"),
        }
    }
}

// --- Single-decoder policy ---

#[test]
fn second_open_before_close_emits_source_already_open() {
    let (engine, rx) = Engine::new();
    let (sid1, rid1) = engine.open(asset("testsrc_h264.mp4"));
    recv_opened(&rx, rid1);

    let (_sid2, rid2) = engine.open(asset("testsrc_h264.mp4"));
    match rx.recv_timeout(Duration::from_secs(5)).expect("event") {
        EngineEvent::Error {
            error: EngineError::SourceAlreadyOpen(id),
            request_id: Some(r),
            ..
        } => {
            assert_eq!(id, sid1);
            assert_eq!(r, rid2);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn after_close_new_open_uses_new_source_id_and_works() {
    let (engine, rx) = Engine::new();
    let sid1 = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    engine.close(sid1);
    match rx.recv_timeout(Duration::from_secs(5)).expect("closed") {
        EngineEvent::Closed { source_id } => assert_eq!(source_id, sid1),
        other => panic!("{other:?}"),
    }

    let (sid2, rid2) = engine.open(asset("testsrc_h264.mp4"));
    assert_ne!(sid2, sid1);
    recv_opened(&rx, rid2);

    let t = Rational::new_raw(1, 1);
    let req = engine.seek_exact(sid2, t);
    let frame = expect_frame_with_req(&rx, sid2, req, t);
    assert!((frame.pts.as_f64() - 1.0).abs() < 0.05);
}

// --- Shutdown ---

#[test]
fn drop_engine_disconnects_event_receiver() {
    let (engine, rx) = Engine::new();
    drop(engine);
    match rx.recv_timeout(Duration::from_secs(3)) {
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
        Ok(ev) => panic!("unexpected event after drop: {ev:?}"),
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
            panic!("expected disconnected receiver after engine drop")
        }
    }
}

// --- seek_exact ---

#[test]
fn seek_exact_zero_first_picture_at_or_after_zero() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    let target = Rational::new_raw(0, 1);
    let req = engine.seek_exact(sid, target);
    let f = expect_frame_with_req(&rx, sid, req, target);
    let z = Rational::new_raw(0, 1);
    assert!(f.pts.ge(z), "{:?}", f.pts);
}

#[test]
fn seek_exact_two_seconds_matches_decoder_expectation() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    let target = Rational::new_raw(2, 1);
    let req = engine.seek_exact(sid, target);
    let f = expect_frame_with_req(&rx, sid, req, target);
    assert!((f.pts.as_f64() - 2.0).abs() < 1e-3);
}

#[test]
fn seek_exact_fractional_seconds() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    let target = Rational::new_raw(10, 3);
    let req = engine.seek_exact(sid, target);
    let f = expect_frame_with_req(&rx, sid, req, target);
    assert!(f.pts.ge(target));
}

#[test]
fn seek_exact_two_seconds_bframes_fixture() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_bframes.mp4");
    let target = Rational::new_raw(2, 1);
    let req = engine.seek_exact(sid, target);
    let f = expect_frame_with_req(&rx, sid, req, target);
    assert!((f.pts.as_f64() - 2.0).abs() < 1e-3);
}

#[test]
fn seek_exact_rational_equivalent_times_land_same_pts_family() {
    let (e1, r1) = Engine::new();
    let s1 = open_and_handshake(&e1, &r1, "testsrc_h264.mp4");
    let req_a = e1.seek_exact(s1, Rational::new_raw(60, 30));
    let frame_a = expect_frame_with_req(&r1, s1, req_a, Rational::new_raw(2, 1));

    let (e2, r2) = Engine::new();
    let s2 = open_and_handshake(&e2, &r2, "testsrc_h264.mp4");
    let req_b = e2.seek_exact(s2, Rational::new_raw(2, 1));
    let frame_b = expect_frame_with_req(&r2, s2, req_b, Rational::new_raw(2, 1));

    assert_eq!(frame_a.pts, frame_b.pts);
}

#[test]
fn seek_exact_chained_multiple_targets_in_order() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");

    for t in [
        Rational::new_raw(3, 1),
        Rational::new_raw(1, 1),
        Rational::new_raw(4, 1),
    ] {
        let req = engine.seek_exact(sid, t);
        expect_frame_with_req(&rx, sid, req, t);
    }
}

#[test]
fn seek_exact_last_gop_then_next_frame_advances_pts() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    let target = Rational::new_raw(9, 2);
    let req0 = engine.seek_exact(sid, target);
    let f0 = expect_frame_with_req(&rx, sid, req0, target);

    let req1 = engine.next_frame(sid);
    let f1 = expect_frame_with_req(&rx, sid, req1, f0.pts);
    assert!(f1.pts.ge(f0.pts));
}

#[test]
fn seek_exact_past_duration_emits_eof_with_request_id() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    let far = Rational::new_raw(1_000_000, 1);
    let req = engine.seek_exact(sid, far);
    expect_eof_with_req(&rx, sid, req);
}

// --- next_frame / EOF ---

#[test]
fn next_frames_after_seek_are_strictly_non_decreasing_pts() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    let t = Rational::new_raw(2, 1);
    let _ = engine.seek_exact(sid, t);
    let mut last = match rx.recv_timeout(Duration::from_secs(5)).expect("seek result") {
        EngineEvent::Frame { frame, .. } => frame.pts,
        other => panic!("unexpected {other:?}"),
    };

    for _ in 0..3 {
        engine.next_frame(sid);
        let pts = match rx.recv_timeout(Duration::from_secs(5)).expect("next") {
            EngineEvent::Frame { frame, .. } => frame.pts,
            other => panic!("unexpected {other:?}"),
        };
        assert!(pts.ge(last), "{:?} -> {:?}", last, pts);
        last = pts;
    }
}

#[test]
fn decode_full_h264_clip_counts_frames_at_30fps() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");

    let mut n = 0u32;
    let mut last_pts = Rational::new_raw(i64::MIN, 1);

    loop {
        let req = engine.next_frame(sid);
        match rx.recv_timeout(Duration::from_secs(5)).expect("tick") {
            EngineEvent::Frame {
                frame,
                request_id: Some(r),
                ..
            } => {
                assert_eq!(r, req);
                if n > 0 {
                    assert!(frame.pts.ge(last_pts));
                }
                last_pts = frame.pts;
                n += 1;
            }
            EngineEvent::Eof {
                request_id: Some(r), ..
            } => {
                assert_eq!(r, req);
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(n, 150, "5s * 30fps");
}

#[test]
fn eof_then_next_frame_stays_eof_until_seek() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");

    loop {
        let req = engine.next_frame(sid);
        match rx.recv_timeout(Duration::from_secs(5)).expect("drain") {
            EngineEvent::Frame { .. } => {}
            EngineEvent::Eof {
                request_id: Some(r), ..
            } => {
                assert_eq!(r, req);
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    let req2 = engine.next_frame(sid);
    match rx.recv_timeout(Duration::from_secs(5)).expect("second eof") {
        EngineEvent::Eof {
            request_id: Some(r), ..
        } => assert_eq!(r, req2),
        other => panic!("unexpected {other:?}"),
    }

    let t = Rational::new_raw(1, 1);
    let req3 = engine.seek_exact(sid, t);
    expect_frame_with_req(&rx, sid, req3, t);
}

// --- Close / errors ---

#[test]
fn close_then_next_emits_source_not_found() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    engine.close(sid);
    match rx.recv_timeout(Duration::from_secs(5)).expect("closed") {
        EngineEvent::Closed { source_id } => assert_eq!(source_id, sid),
        other => panic!("unexpected {other:?}"),
    }

    engine.next_frame(sid);
    match rx.recv_timeout(Duration::from_secs(5)).expect("err") {
        EngineEvent::Error {
            error: EngineError::SourceNotFound(id),
            request_id: Some(_),
            ..
        } => assert_eq!(id, sid),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn close_is_idempotent_no_second_closed_event() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    engine.close(sid);
    recv_closed(&rx, sid);
    engine.close(sid);
    drain_available(&rx, 80);
}

fn recv_closed(rx: &EventReceiver, sid: SourceId) {
    match rx.recv_timeout(Duration::from_secs(5)).expect("closed") {
        EngineEvent::Closed { source_id } => assert_eq!(source_id, sid),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn seek_exact_unknown_source_carries_request_id_on_error() {
    let (engine, rx) = Engine::new();
    let ghost = SourceId(999);
    let req = engine.seek_exact(ghost, Rational::new_raw(1, 1));
    match rx.recv_timeout(Duration::from_secs(5)).expect("err") {
        EngineEvent::Error {
            source_id: Some(sid),
            error: EngineError::SourceNotFound(id),
            request_id: Some(r),
        } => {
            assert_eq!(sid, ghost);
            assert_eq!(id, ghost);
            assert_eq!(r, req);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn next_frame_unknown_source_carries_request_id_on_error() {
    let (engine, rx) = Engine::new();
    let ghost = SourceId(888);
    let req = engine.next_frame(ghost);
    match rx.recv_timeout(Duration::from_secs(5)).expect("err") {
        EngineEvent::Error {
            error: EngineError::SourceNotFound(id),
            request_id: Some(r),
            ..
        } => {
            assert_eq!(id, ghost);
            assert_eq!(r, req);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn back_to_back_commands_get_distinct_request_ids() {
    let (engine, rx) = Engine::new();
    let (sid, r_open) = engine.open(asset("testsrc_h264.mp4"));
    recv_opened(&rx, r_open);
    let r_seek = engine.seek_exact(sid, Rational::new_raw(0, 1));
    let r_next = engine.next_frame(sid);
    assert_ne!(r_open, r_seek);
    assert_ne!(r_seek, r_next);
}

// --- seek_scrub ---

#[test]
fn scrub_mid_gop_snaps_near_keyframe_before_target() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");
    engine.seek_scrub(sid, Rational::new_raw(5, 2));
    match rx.recv_timeout(Duration::from_secs(5)).expect("scrub") {
        EngineEvent::Frame {
            frame,
            request_id: None,
            ..
        } => {
            let two = Rational::new_raw(2, 1);
            assert!(
                frame.pts.ge(two) && !frame.pts.ge(Rational::new_raw(3, 1)),
                "snap near 2s, got {:?}",
                frame.pts
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn scrub_burst_coalesces_to_few_decodes_and_ends_near_last_target() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");

    for i in 1..=20 {
        engine.seek_scrub(sid, Rational::new_raw(i, 10));
    }

    let mut scrub_frames = 0u32;
    let mut last_pts = Rational::new_raw(0, 1);
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(EngineEvent::Frame {
                request_id: None,
                frame,
                ..
            }) => {
                scrub_frames += 1;
                last_pts = frame.pts;
            }
            Ok(EngineEvent::Error { .. }) => panic!("unexpected scrub error"),
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    assert!(scrub_frames <= 20);
    assert!(
        scrub_frames < 10,
        "coalescing should drop most redundant scrub wakes, got {scrub_frames} frames"
    );
    assert!(
        last_pts.ge(Rational::new_raw(15, 10)),
        "pts should move toward end of burst, got {:?}",
        last_pts
    );
}

#[test]
fn scrub_after_seek_exact_still_delivers_exact_then_scrub_frames() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");

    let two = Rational::new_raw(2, 1);
    let rid_ex = engine.seek_exact(sid, two);
    engine.seek_scrub(sid, Rational::new_raw(4, 1));

    let mut saw_exact = false;
    let mut saw_scrub = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !(saw_exact && saw_scrub) && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_secs(2)).expect("recv") {
            EngineEvent::Frame {
                frame,
                request_id: Some(r),
                ..
            } if r == rid_ex => {
                assert!(frame.pts.ge(two), "{:?}", frame.pts);
                assert!((frame.pts.as_f64() - 2.0).abs() < 0.05, "{:?}", frame.pts);
                saw_exact = true;
            }
            EngineEvent::Frame {
                frame,
                request_id: None,
                ..
            } => {
                assert!(
                    frame.pts.ge(Rational::new_raw(35, 10)),
                    "scrub ~4s should land past ~3.5s, got {:?}",
                    frame.pts
                );
                saw_scrub = true;
            }
            EngineEvent::Error { error, .. } => panic!("unexpected error: {error}"),
            _ => {}
        }
    }

    assert!(saw_exact && saw_scrub);
}

#[test]
fn scrub_without_open_emits_source_not_found_without_request_id() {
    let (engine, rx) = Engine::new();
    engine.seek_scrub(SourceId(1), Rational::new_raw(1, 1));
    match rx.recv_timeout(Duration::from_secs(3)).expect("event") {
        EngineEvent::Error {
            error: EngineError::SourceNotFound(id),
            request_id: None,
            ..
        } => assert_eq!(id, SourceId(1)),
        other => panic!("unexpected {other:?}"),
    }
}

// --- Cloned handle / concurrency ---

#[test]
fn cloned_engine_handle_submits_same_worker_and_correlates_requests() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");

    let e2 = engine.clone();
    let r_a = engine.seek_exact(sid, Rational::new_raw(1, 1));
    let r_b = e2.seek_exact(sid, Rational::new_raw(3, 1));

    let mut got = Vec::new();
    for _ in 0..2 {
        match rx.recv_timeout(Duration::from_secs(5)).expect("frame") {
            EngineEvent::Frame {
                request_id: Some(r),
                frame,
                ..
            } => got.push((r, frame.pts)),
            other => panic!("unexpected {other:?}"),
        }
    }

    let ids: std::collections::HashSet<_> = got.iter().map(|(r, _)| *r).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&r_a));
    assert!(ids.contains(&r_b));
}

#[test]
fn concurrent_engine_clones_race_two_seeks_both_results_valid() {
    let (engine, rx) = Engine::new();
    let sid = open_and_handshake(&engine, &rx, "testsrc_h264.mp4");

    let e_a = engine.clone();
    let e_b = engine.clone();

    let h_four = thread::spawn(move || e_a.seek_exact(sid, Rational::new_raw(4, 1)));
    let h_one = thread::spawn(move || e_b.seek_exact(sid, Rational::new_raw(1, 1)));
    let r_four = h_four.join().expect("j1");
    let r_one = h_one.join().expect("j2");
    assert_ne!(r_four, r_one);

    let mut pts_by_req = std::collections::HashMap::new();
    for _ in 0..2 {
        match rx.recv_timeout(Duration::from_secs(5)).expect("ev") {
            EngineEvent::Frame {
                request_id: Some(r),
                frame,
                ..
            } => {
                pts_by_req.insert(r, frame.pts);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    let p_four = *pts_by_req.get(&r_four).expect("four second seek");
    let p_one = *pts_by_req.get(&r_one).expect("one second seek");
    assert!(p_four.ge(Rational::new_raw(4, 1)), "{p_four:?}");
    assert!(p_one.ge(Rational::new_raw(1, 1)), "{p_one:?}");
}
