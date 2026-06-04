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

/// Two imports at once must both build (across the SW + HW lanes) and both be
/// adopted — i.e. the lane pool work-steals and doesn't deadlock.
#[test]
fn multiple_imports_build_in_parallel() {
    let Some(src) = sibling_asset("testsrc_h264.mp4") else {
        return;
    };

    // Distinct source paths -> distinct content-addressed proxy paths, so the two
    // builds don't collide on one output file.
    let tmp = std::env::temp_dir();
    let a_src = tmp.join("cutlass_multi_a.mp4");
    let b_src = tmp.join("cutlass_multi_b.mp4");
    std::fs::copy(&src, &a_src).expect("copy a");
    std::fs::copy(&src, &b_src).expect("copy b");

    let a = MediaSource::new(a_src.clone(), 320, 240, Rational::FPS_30, 150, false);
    let b = MediaSource::new(b_src.clone(), 320, 240, Rational::FPS_30, 150, false);
    let pa = proxy_path(&a_src, 1080);
    let pb = proxy_path(&b_src, 1080);
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    let mut pool = MediaPool::new();
    pool.open(&a).expect("open a");
    pool.open(&b).expect("open b");
    pool.request_proxy(&a);
    pool.request_proxy(&b);
    // Bump b under the (hypothetical) playhead; must not deadlock or lose a.
    pool.prioritize_proxy(b.id);

    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        pool.poll_proxies();
        let a_ready = matches!(pool.proxy_status(a.id), Some(ProxyStatus::Ready(_)));
        let b_ready = matches!(pool.proxy_status(b.id), Some(ProxyStatus::Ready(_)));
        if let Some(ProxyStatus::Failed(e)) = pool.proxy_status(a.id) {
            panic!("proxy a failed: {e}");
        }
        if let Some(ProxyStatus::Failed(e)) = pool.proxy_status(b.id) {
            panic!("proxy b failed: {e}");
        }
        if a_ready && b_ready {
            break;
        }
        assert!(Instant::now() < deadline, "parallel proxy builds timed out");
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(pool.frame(a.id, 30).is_ok(), "frame via proxy a");
    assert!(pool.frame(b.id, 30).is_ok(), "frame via proxy b");

    for p in [&pa, &pb, &a_src, &b_src] {
        let _ = std::fs::remove_file(p);
    }
}
