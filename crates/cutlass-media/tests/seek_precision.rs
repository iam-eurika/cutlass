//! Integration tests for `MediaSource::decode_frame_at`.
//!
//! Contract under test: `decode_frame_at(target)` returns the frame whose
//! presentation interval contains `target` — i.e. the frame with the
//! latest PTS ≤ target. PTS is exposed as a `Rational64` in seconds, so
//! exact-grid asserts use `assert_eq!` on the rational; only inherently
//! lossy quantities (container-reported duration, off-grid midpoints
//! near EOF) get a tolerance.
//!
//! Fixtures are built once per test process by `tests/fixtures/build.sh`.
//! They are tiny synthetic clips chosen to exercise specific failure
//! modes:
//!
//! - `cfr_30.mp4` — clean integer-grid baseline.
//! - `cfr_2997.mp4` — 30000/1001 fractional rate, catches any float-based
//!   PTS arithmetic.
//! - `bframes_30.mp4` — verifies decoded frames come out in display order
//!   despite encode-order B-frame reorder.
//! - `cfr_50_tb50.mp4` — small `1/50` time_base, catches code that
//!   assumes a large time_base denominator.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use cutlass_media::{HwAccel, MediaSource};
use num_rational::Rational64;

// ---------------------------------------------------------------------
// Setup helpers
// ---------------------------------------------------------------------

static FIXTURES_BUILT: OnceLock<()> = OnceLock::new();

/// Run `tests/fixtures/build.sh` exactly once per test process. Tests
/// run in parallel; `OnceLock` makes that safe and avoids 4 redundant
/// ffmpeg spawns.
fn ensure_fixtures() {
    FIXTURES_BUILT.get_or_init(|| {
        let script = fixture_dir().join("build.sh");
        let output = Command::new("bash")
            .arg(&script)
            .output()
            .unwrap_or_else(|e| {
                panic!(
                    "failed to spawn fixtures build.sh ({}): {e}",
                    script.display()
                )
            });
        assert!(
            output.status.success(),
            "fixtures build.sh ({}) failed with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            script.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    });
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_path(name: &str) -> PathBuf {
    fixture_dir().join(name)
}

// ---------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------

/// Render a `Rational64` of seconds as milliseconds for human-readable
/// failure messages. Tests still assert on the exact rational where
/// possible; this is just for the panic text.
fn ms(r: Rational64) -> f64 {
    *r.numer() as f64 / *r.denom() as f64 * 1000.0
}

/// Inclusive tolerance check on a `Rational64`. Used only for genuinely
/// lossy comparisons (e.g. last-frame near container-reported duration).
fn within(landed: Rational64, expected: Rational64, tol: Rational64, ctx: &str) {
    let diff = if landed >= expected {
        landed - expected
    } else {
        expected - landed
    };
    assert!(
        diff <= tol,
        "{ctx}: landed {:.3} ms, expected {:.3} ms ± {:.3} ms (diff {:.3} ms)",
        ms(landed),
        ms(expected),
        ms(tol),
        ms(diff),
    );
}

/// Open a fixture, decode at `target`, and assert the returned PTS is
/// exactly `expected`. The error message names the fixture, the target,
/// and both PTS sides so a regression points at the broken case.
fn assert_lands_exactly(src: &mut MediaSource, fixture: &str, target: Rational64, expected: Rational64) {
    let frame = src
        .decode_frame_at(target)
        .unwrap_or_else(|e| panic!("{fixture}: decode_frame_at({:?}) failed: {e}", target));
    assert_eq!(
        frame.pts, expected,
        "{fixture}: target {}/{} ({:.3} ms) — landed {}/{} ({:.3} ms), expected {}/{} ({:.3} ms)",
        target.numer(),
        target.denom(),
        ms(target),
        frame.pts.numer(),
        frame.pts.denom(),
        ms(frame.pts),
        expected.numer(),
        expected.denom(),
        ms(expected),
    );
}

fn open_fixture(name: &str) -> MediaSource {
    ensure_fixtures();
    let path = fixture_path(name);
    MediaSource::open(&path)
        .unwrap_or_else(|e| panic!("opening fixture {} failed: {e}", path.display()))
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[test]
fn cfr_30_lands_exactly_on_frame_boundaries() {
    let mut src = open_fixture("cfr_30.mp4");
    for n in [0_i64, 1, 5, 29] {
        let pts = Rational64::new(n, 30);
        assert_lands_exactly(&mut src, "cfr_30.mp4", pts, pts);
    }
}

#[test]
fn cfr_30_midpoint_returns_lower_frame() {
    // Halfway between frame 1 (1/30) and frame 2 (2/30). The frame whose
    // *interval* contains the target is frame 1 — its interval covers
    // [1/30, 2/30). Returning frame 2 here would mean the seek code is
    // rounding to the nearest frame instead of the floor.
    let mut src = open_fixture("cfr_30.mp4");
    let target = Rational64::new(1, 30) + Rational64::new(1, 60);
    let expected = Rational64::new(1, 30);
    assert_lands_exactly(&mut src, "cfr_30.mp4 (midpoint 1.5/30)", target, expected);
}

#[test]
fn cfr_2997_avoids_float_drift() {
    // The whole point of carrying PTS as Rational64 is that frame 100 of
    // a 30000/1001 stream lands at *exactly* 100*1001/30000 s. Any
    // intermediate `f64` round-trip would shift this to e.g.
    // 3.337000000000xxx and break the equality.
    let mut src = open_fixture("cfr_2997.mp4");

    let exact_100 = Rational64::new(100 * 1001, 30000);
    assert_lands_exactly(&mut src, "cfr_2997.mp4 (frame 100)", exact_100, exact_100);

    // Midpoint between frame 99 and frame 100. Floor rule → frame 99.
    let pts_99 = Rational64::new(99 * 1001, 30000);
    let pts_100 = Rational64::new(100 * 1001, 30000);
    let midpoint = (pts_99 + pts_100) / Rational64::from_integer(2);
    assert_lands_exactly(
        &mut src,
        "cfr_2997.mp4 (midpoint 99.5)",
        midpoint,
        pts_99,
    );
}

#[test]
fn bframes_30_returns_display_order() {
    // Non-monotonic order forces the seek path to flush + re-decode and
    // verifies the decoder hands frames back in display order rather
    // than encode order. With B-frames, encode order ≠ display order.
    let mut src = open_fixture("bframes_30.mp4");
    for n in [29_i64, 5, 17, 0] {
        let pts = Rational64::new(n, 30);
        assert_lands_exactly(&mut src, "bframes_30.mp4", pts, pts);
    }
}

#[test]
fn tb50_handles_small_timebase() {
    // time_base is 1/50, so frame N's stream-tick PTS is just N. Catches
    // any code that assumes a large denominator (e.g. `* 1000` instead
    // of using the actual time_base).
    let mut src = open_fixture("cfr_50_tb50.mp4");
    for n in [0_i64, 1, 25, 50, 99] {
        let pts = Rational64::new(n, 50);
        assert_lands_exactly(&mut src, "cfr_50_tb50.mp4", pts, pts);
    }
}

#[test]
fn t0_returns_first_frame() {
    let zero = Rational64::from_integer(0);
    for fixture in [
        "cfr_30.mp4",
        "cfr_2997.mp4",
        "bframes_30.mp4",
        "cfr_50_tb50.mp4",
    ] {
        let mut src = open_fixture(fixture);
        assert_lands_exactly(&mut src, fixture, zero, zero);
    }
}

#[test]
fn near_end_returns_last_decodable_frame() {
    // Container duration is reported in microseconds, so the expectation
    // `dur - frame_period` is only accurate to within container-rounding
    // (sub-microsecond). A `frame_period / 2` tolerance absorbs that
    // rounding but still catches a real off-by-one-frame regression
    // (which would land a full frame_period away).
    for fixture in [
        "cfr_30.mp4",
        "cfr_2997.mp4",
        "bframes_30.mp4",
        "cfr_50_tb50.mp4",
    ] {
        let mut src = open_fixture(fixture);
        let info = src.info().clone();
        let dur = info.duration;
        let fps = info
            .video
            .as_ref()
            .and_then(|v| v.frame_rate)
            .unwrap_or_else(|| panic!("{fixture}: probe reported no avg_frame_rate"));
        let frame_period = fps.recip();
        let half_frame = frame_period / Rational64::from_integer(2);

        // Aim at the middle of the last frame's interval, so the
        // expectation is unambiguously the final frame.
        let target = dur - half_frame;
        let expected = dur - frame_period;
        let frame = src
            .decode_frame_at(target)
            .unwrap_or_else(|e| panic!("{fixture}: decode near end failed: {e}"));
        within(
            frame.pts,
            expected,
            half_frame,
            &format!("{fixture} near-end"),
        );
        assert!(
            frame.pts <= dur,
            "{fixture}: landed past container duration ({:.3} ms > {:.3} ms)",
            ms(frame.pts),
            ms(dur),
        );
    }
}

/// On macOS, VideoToolbox must engage for vanilla H.264 — every fixture
/// in this suite is x264-encoded MP4, which VT supports across every
/// supported macOS version. A silent regression here (e.g. someone
/// accidentally drops the `get_format` callback or the `hw_device_ctx`
/// assignment) would still pass the seek-precision tests because the
/// decoder would just fall through to software, but we'd lose a major
/// performance win without noticing. This test is the canary.
#[cfg(target_os = "macos")]
#[test]
fn videotoolbox_engages_on_h264() {
    for fixture in [
        "cfr_30.mp4",
        "cfr_2997.mp4",
        "bframes_30.mp4",
        "cfr_50_tb50.mp4",
    ] {
        let src = open_fixture(fixture);
        assert_eq!(
            src.hw_accel(),
            HwAccel::VideoToolbox,
            "{fixture}: expected VideoToolbox to engage on macOS for H.264"
        );
    }
}

#[test]
fn random_scrub_pattern_warm_decoder() {
    // Open once, scrub repeatedly. This is the timeline-scrubbing
    // acceptance test: the decoder must keep returning exact frame PTS
    // across repeated forward + backward seeks without leaking stale
    // state between calls. Sequence is generated by a deterministic
    // 64-bit LCG (Knuth's MMIX constants) so any failure repros.
    let mut src = open_fixture("cfr_30.mp4");

    const FRAME_COUNT: u64 = 60; // 2 s @ 30 fps
    const ITERATIONS: usize = 30;
    let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
    fn lcg(s: &mut u64) -> u64 {
        *s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *s
    }

    for i in 0..ITERATIONS {
        let n = (lcg(&mut state) % FRAME_COUNT) as i64;
        let pts = Rational64::new(n, 30);
        let frame = src.decode_frame_at(pts).unwrap_or_else(|e| {
            panic!("scrub iter {i} (frame {n}): decode_frame_at failed: {e}")
        });
        assert_eq!(
            frame.pts, pts,
            "scrub iter {i}: requested frame {n} ({}/{} = {:.3} ms), landed {}/{} ({:.3} ms)",
            pts.numer(),
            pts.denom(),
            ms(pts),
            frame.pts.numer(),
            frame.pts.denom(),
            ms(frame.pts),
        );
    }
}
