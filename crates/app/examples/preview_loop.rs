//! Timeline → engine → renderer preview (same as the `app` binary).
//!
//! ```text
//! cargo run -p app --example preview_loop
//! ```

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let media = std::env::args().nth(1).map(std::path::PathBuf::from);
    app::run_preview_cli(media);
}
