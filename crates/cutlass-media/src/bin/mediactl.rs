//! `mediactl` — the headless test harness for `cutlass-media`.
//!
//! Subcommands:
//! - `probe`  open a file, print stream info
//! - `frame`  decode one frame at a given time, write PNG

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cutlass_media::{MediaSource, time::parse as parse_time};
use image::{ImageBuffer, Rgba};
use num_rational::Rational64;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "mediactl", about = "Cutlass media inspection tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Open a file and print what we found in it.
    Probe {
        /// Path to a media file.
        file: PathBuf,
    },

    /// Decode a single frame at the given time and write it to disk.
    Frame {
        /// Path to a media file.
        file: PathBuf,

        /// Target time. Accepts `5.5`, `5500ms`, or `HH:MM:SS[.fff]`.
        #[arg(long)]
        at: String,

        /// Output PNG path.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().cmd {
        Cmd::Probe { file } => probe(file),
        Cmd::Frame { file, at, out } => frame(file, at, out),
    }
}

fn probe(file: PathBuf) -> Result<()> {
    let src = MediaSource::open(&file)?;
    let info = src.info();
    let hw = src.hw_accel();

    println!("file:     {}", info.path.display());
    println!("format:   {}", info.format_name);
    println!("duration: {}", fmt_seconds(info.duration));

    if let Some(v) = &info.video {
        println!();
        println!("video:");
        println!("  index:      {}", v.stream_index);
        println!("  codec:      {}", v.codec);
        println!("  size:       {}x{}", v.width, v.height);
        println!("  pix_fmt:    {}", v.pix_fmt);
        println!("  hwaccel:    {}", hw.as_str());
        println!(
            "  fps:        {}",
            v.frame_rate
                .map(|r| format!("{:.4} ({}/{})", r_to_f64(r), r.numer(), r.denom()))
                .unwrap_or_else(|| "VFR".into())
        );
        println!(
            "  time_base:  {}/{}",
            v.time_base.numer(),
            v.time_base.denom()
        );
        println!("  rotation:   {}°", v.rotation);
    } else {
        println!("\nvideo:   (none)");
    }

    if let Some(a) = &info.audio {
        println!();
        println!("audio:");
        println!("  index:      {}", a.stream_index);
        println!("  codec:      {}", a.codec);
        println!("  rate:       {} Hz", a.sample_rate);
        println!("  channels:   {}", a.channels);
        println!("  sample_fmt: {}", a.sample_fmt);
        println!(
            "  time_base:  {}/{}",
            a.time_base.numer(),
            a.time_base.denom()
        );
    } else {
        println!("\naudio:   (none)");
    }

    Ok(())
}

fn frame(file: PathBuf, at: String, out: PathBuf) -> Result<()> {
    let target = parse_time(&at).with_context(|| format!("parsing --at {at:?}"))?;
    let total = Instant::now();

    let open_start = Instant::now();
    let mut src = MediaSource::open(&file)?;
    let open_ms = open_start.elapsed().as_secs_f64() * 1000.0;
    let hw = src.hw_accel();

    let decode_start = Instant::now();
    let frame = src.decode_frame_at(target)?;
    let decode_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

    let write_start = Instant::now();
    let buf: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(frame.width, frame.height, frame.rgba)
            .ok_or_else(|| anyhow::anyhow!("rgba buffer size mismatch"))?;
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }
    buf.save(&out)
        .with_context(|| format!("writing {}", out.display()))?;
    let write_ms = write_start.elapsed().as_secs_f64() * 1000.0;

    println!(
        "wrote {} ({}x{}) — target {} → landed {} (Δ {:.3}ms)",
        out.display(),
        frame.width,
        frame.height,
        fmt_seconds(target),
        fmt_seconds(frame.pts),
        (r_to_f64(frame.pts) - r_to_f64(target)) * 1000.0
    );
    println!(
        "timings: open {open_ms:.1}ms · decode {decode_ms:.1}ms · png {write_ms:.1}ms · total {:.1}ms (hwaccel: {})",
        total.elapsed().as_secs_f64() * 1000.0,
        hw.as_str(),
    );
    Ok(())
}

fn fmt_seconds(r: Rational64) -> String {
    let secs = r_to_f64(r);
    let total_ms = (secs * 1000.0).round() as i64;
    let h = total_ms / 3_600_000;
    let m = (total_ms / 60_000) % 60;
    let s = (total_ms / 1000) % 60;
    let ms = total_ms % 1000;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

fn r_to_f64(r: Rational64) -> f64 {
    *r.numer() as f64 / *r.denom() as f64
}
