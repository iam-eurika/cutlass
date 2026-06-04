//! End-to-end proxy tier: import a real file, let the background worker build a
//! proxy, and confirm the pool adopts it and serves frames through it.
//!
//! Skips when the shared sibling test asset isn't present.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cutlass_engines::{proxy_path, MediaPool, ProxyStatus};
use cutlass_models::{MediaSource, Rational};

fn sibling_asset(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../cutlass-main/crates/decoder/tests/assets")
        .join(name);
    path.exists().then_some(path)
}

#[test]
fn proxy_builds_in_background_and_is_adopted() {
    let Some(path) = sibling_asset("testsrc_h264.mp4") else {
        return;
    };
    // testsrc_h264.mp4: 320x240, 30fps, 150 frames (5s).
    let media = MediaSource::new(path.clone(), 320, 240, Rational::FPS_30, 150, false);

    // Start clean so we exercise an actual build, not cross-run adoption.
    let pp = proxy_path(&path, 1080);
    let _ = std::fs::remove_file(&pp);

    let mut pool = MediaPool::new();
    pool.open(&media).expect("open source");
    pool.request_proxy(&media);

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        pool.poll_proxies();
        match pool.proxy_status(media.id) {
            Some(ProxyStatus::Ready(_)) => break,
            Some(ProxyStatus::Failed(e)) => panic!("proxy build failed: {e}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "proxy build timed out");
        std::thread::sleep(Duration::from_millis(50));
    }

    // With the proxy ready, a frame request is served through the proxy reader.
    let frame = pool.frame(media.id, 60).expect("frame via proxy");
    assert!(frame.width > 0 && !frame.planes.is_empty());

    let _ = std::fs::remove_file(&pp);
}
