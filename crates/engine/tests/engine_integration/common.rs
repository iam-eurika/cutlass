//! Shared helpers for `engine_integration` integration test crate.

use std::path::PathBuf;
use std::time::Duration;

use decoder::{DecodedVideoFrame, Rational};
use engine::{Engine, EngineEvent, EventReceiver, RequestId, SourceId};

pub fn asset(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
        .join(name)
}

pub fn recv_opened(rx: &EventReceiver, expect_rid: RequestId) -> SourceId {
    match rx.recv_timeout(Duration::from_secs(5)).expect("recv Opened") {
        EngineEvent::Opened {
            source_id,
            request_id,
            ..
        } => {
            assert_eq!(request_id, expect_rid);
            source_id
        }
        other => panic!("expected Opened, got {other:?}"),
    }
}

/// Open `name`, assert `Opened` matches `(source_id, request_id)` from [`Engine::open`].
pub fn open_and_handshake(engine: &Engine, rx: &EventReceiver, name: &str) -> SourceId {
    let (sid, rid) = engine.open(asset(name));
    assert_eq!(recv_opened(rx, rid), sid);
    sid
}

pub fn expect_frame_with_req(
    rx: &EventReceiver,
    sid: SourceId,
    expect_req: RequestId,
    min_pts: Rational,
) -> DecodedVideoFrame {
    match rx.recv_timeout(Duration::from_secs(5)).expect("frame event") {
        EngineEvent::Frame {
            source_id,
            frame,
            request_id: Some(r),
        } => {
            assert_eq!(source_id, sid);
            assert_eq!(r, expect_req);
            assert!(
                frame.pts.ge(min_pts),
                "pts {:?} should be >= {:?}",
                frame.pts,
                min_pts
            );
            frame
        }
        other => panic!("expected Frame with request_id, got {other:?}"),
    }
}

pub fn expect_eof_with_req(rx: &EventReceiver, sid: SourceId, expect_req: RequestId) {
    match rx.recv_timeout(Duration::from_secs(5)).expect("eof") {
        EngineEvent::Eof {
            source_id,
            request_id: Some(r),
        } => {
            assert_eq!(source_id, sid);
            assert_eq!(r, expect_req);
        }
        other => panic!("expected Eof with request_id, got {other:?}"),
    }
}

/// Non-blocking drain for a short window (tests that expect silence).
pub fn drain_available(rx: &EventReceiver, ms: u64) {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(ev) => panic!("unexpected extra event: {ev:?}"),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }
    }
}
