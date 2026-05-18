//! Cutlass app binary: timeline-driven preview smoke test.
//!
//! ```text
//! cargo run -p app
//! cargo run -p app -- /path/to/video.mp4
//! ```

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let media = std::env::args().nth(1).map(PathBuf::from);
    app::run_preview_cli(media);
}
