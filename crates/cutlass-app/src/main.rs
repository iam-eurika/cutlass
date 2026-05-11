use std::env;

use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

fn main() -> anyhow::Result<()> {
    fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    info!("cutlass starting");

    // Until the Slint shell lands, the app binary just probes a file
    // passed on the command line. This keeps the end-to-end wiring
    // exercised while the renderer is being built.
    let Some(path) = env::args().nth(1) else {
        warn!("usage: cutlass <media-file>   (no file given, exiting)");
        return Ok(());
    };

    let src = cutlass_media::MediaSource::open(&path)?;
    info!(?path, info = ?src.info(), "probed");
    Ok(())
}
