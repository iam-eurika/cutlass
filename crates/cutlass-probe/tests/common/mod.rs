//! Shared helpers for `cutlass-probe` integration tests.

use std::path::PathBuf;

pub fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local-assets/assets")
}

pub fn small_video_asset() -> Option<PathBuf> {
    let dir = assets_dir();
    for name in [
        "15531444_1920_1080_24fps.mp4",
        "6137050-hd_1920_1080_24fps.mp4",
        "15604765_1920_1080_24fps.mp4",
    ] {
        let path = dir.join(name);
        if path.exists() {
            return Some(path);
        }
    }
    any_mp4_in_assets()
}

pub fn any_mp4_in_assets() -> Option<PathBuf> {
    let dir = assets_dir();
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|ext| ext == "mp4"))
}
