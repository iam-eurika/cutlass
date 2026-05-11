//! Single-frame decode + frame-accurate seek.
//!
//! Algorithm (the "tiered seek" from the project plan, single-shot
//! variant for Phase 1):
//!
//! 1. Convert the target time (in seconds, [`Rational64`]) to ffmpeg's
//!    `AV_TIME_BASE` and to the stream's own time-base.
//! 2. `av_seek_frame` **backwards** to the keyframe at or before target,
//!    then flush the decoder so stale frames don't leak through.
//! 3. Decode forward, keeping the last frame whose PTS is `<= target`.
//! 4. As soon as we see a frame with `pts > target`, the previously
//!    saved frame is the answer (its presentation interval contains the
//!    target). If the seek landed *past* the target (rare), that first
//!    overshooting frame is itself the answer.
//! 5. On EOF, the best frame seen so far wins.
//!
//! Output is RGBA8 (color-converted via `libswscale`) with the source's
//! native dimensions. Color-management beyond bt709 default behavior is
//! deferred to a later phase.
//!
//! # Hardware acceleration
//!
//! On macOS we attach a VideoToolbox `AVHWDeviceContext` to the codec
//! before `avcodec_open2` and install a `get_format` callback that
//! returns `AV_PIX_FMT_VIDEOTOOLBOX` when the codec offers it. Decoded
//! frames then live in GPU memory; before swscale we download them into
//! a system-memory frame with `av_hwframe_transfer_data` (typically
//! NV12 / P010), copying props so PTS survives. Any setup failure
//! (codec without VT support, `av_hwdevice_ctx_create` failing, etc.)
//! silently falls back to the software path, so the decoder always
//! produces a frame.
//!
//! Other platforms currently use the software path; adding NVDEC /
//! VAAPI / D3D11VA later is a matter of teaching `try_init_hw_decoder`
//! to also try those device types.

use std::ffi::c_int;
use std::ptr;

use ffmpeg_next as ff;
use ff::ffi as sys;
use ff::format::Pixel;
use ff::software::scaling::{context::Context as Scaler, flag::Flags};
use num_rational::Rational64;
use tracing::{debug, warn};

use crate::{MediaError, Result, probe::rational_from_ff};

/// Which hardware acceleration backend is active on a decoder, if any.
///
/// Exposed so callers (CLI tools, telemetry) can report what actually
/// got wired up — the decoder is allowed to fall back silently when a
/// requested backend can't be initialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwAccel {
    /// Pure software decode + libswscale.
    None,
    /// macOS VideoToolbox-backed decode; frames are downloaded to
    /// system memory before color conversion.
    VideoToolbox,
}

impl HwAccel {
    pub fn as_str(self) -> &'static str {
        match self {
            HwAccel::None => "none",
            HwAccel::VideoToolbox => "videotoolbox",
        }
    }
}

/// A decoded video frame ready to upload, save, or further process.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed `width * height * 4` bytes, row-major, RGBA8.
    pub rgba: Vec<u8>,
    /// Actual presentation time of the returned frame, in seconds.
    pub pts: Rational64,
}

/// Per-source warm decoder state. Held inside [`crate::MediaSource`] so
/// we don't re-open the codec on every frame request.
pub(crate) struct VideoDecodeState {
    stream_index: usize,
    decoder: ff::decoder::Video,
    /// (input pixel format → RGBA) scaler. Built lazily on the first
    /// frame because hwaccel decoders only reveal the post-download
    /// pixel format after the first transfer; cached and rebuilt only
    /// when the input format changes (rare in practice).
    scaler: Option<(Pixel, Scaler)>,
    width: u32,
    height: u32,
    time_base: Rational64,
    hw_accel: HwAccel,
}

impl VideoDecodeState {
    pub(crate) fn new(ictx: &ff::format::context::Input) -> Result<Option<Self>> {
        let Some(stream) = ictx.streams().best(ff::media::Type::Video) else {
            return Ok(None);
        };
        let stream_index = stream.index();
        let time_base = rational_from_ff(stream.time_base());
        let mut codec_ctx =
            ff::codec::context::Context::from_parameters(stream.parameters())?;

        // `from_parameters` copies codec_id but does NOT populate
        // codec_ctx->codec — that field is only set by avcodec_open2.
        // We need a real AVCodec pointer *before* open so the hwaccel
        // helper can inspect AVCodecHWConfig and we can pass it to
        // open_as ourselves. Look it up by id here.
        let codec_id = codec_ctx.id();
        let codec = ff::decoder::find(codec_id);

        // SAFETY: `codec_ctx` owns the AVCodecContext we mutate; the
        // hwaccel helper only writes set-before-open fields
        // (hw_device_ctx, get_format) and reads from the AVCodec ptr we
        // hand it.
        let hw_accel = match codec {
            Some(c) => unsafe { try_init_hw_decoder(codec_ctx.as_mut_ptr(), c.as_ptr()) },
            None => HwAccel::None,
        };

        // If we found a codec, open with it explicitly (so the
        // hw_device_ctx / get_format we just set actually take effect
        // during avcodec_open2). Otherwise fall back to ffmpeg-next's
        // default lookup-and-open path.
        let decoder = match codec {
            Some(c) => codec_ctx.decoder().open_as(c)?.video()?,
            None => codec_ctx.decoder().video()?,
        };
        let width = decoder.width();
        let height = decoder.height();
        debug!(
            hw_accel = hw_accel.as_str(),
            width, height, "video decoder opened"
        );
        Ok(Some(Self {
            stream_index,
            decoder,
            scaler: None,
            width,
            height,
            time_base,
            hw_accel,
        }))
    }

    pub(crate) fn hw_accel(&self) -> HwAccel {
        self.hw_accel
    }
}

/// Try to attach a hardware acceleration context to the codec context
/// **before** `avcodec_open2`. Returns the backend that was actually
/// installed, or [`HwAccel::None`] when we fell through to software.
///
/// # Safety
/// `codec_ctx` must point to a freshly-created, not-yet-opened
/// `AVCodecContext`. `codec` must point to the `AVCodec` that will be
/// passed to `avcodec_open2` next (we look it up before open because
/// `from_parameters` does not populate `codec_ctx->codec`).
unsafe fn try_init_hw_decoder(
    codec_ctx: *mut sys::AVCodecContext,
    codec: *const sys::AVCodec,
) -> HwAccel {
    if codec_ctx.is_null() || codec.is_null() {
        return HwAccel::None;
    }

    #[cfg(target_os = "macos")]
    {
        if unsafe { try_init_videotoolbox(codec_ctx, codec) } {
            return HwAccel::VideoToolbox;
        }
    }

    let _ = codec;
    HwAccel::None
}

#[cfg(target_os = "macos")]
unsafe fn try_init_videotoolbox(
    codec_ctx: *mut sys::AVCodecContext,
    codec: *const sys::AVCodec,
) -> bool {
    if !unsafe { codec_supports_device(codec, sys::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX) }
    {
        debug!("codec has no VideoToolbox hwconfig; using software decode");
        return false;
    }

    let mut device_ref: *mut sys::AVBufferRef = ptr::null_mut();
    let ret = unsafe {
        sys::av_hwdevice_ctx_create(
            &mut device_ref,
            sys::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
            ptr::null(),
            ptr::null_mut(),
            0,
        )
    };
    if ret < 0 || device_ref.is_null() {
        warn!(ret, "VideoToolbox device init failed; using software decode");
        return false;
    }

    // Hand ownership of the buffer ref to the codec context — it'll be
    // released when the codec context is freed. No extra `av_buffer_ref`
    // is needed because we never use `device_ref` again after this.
    unsafe {
        (*codec_ctx).hw_device_ctx = device_ref;
        (*codec_ctx).get_format = Some(get_videotoolbox_format);
    }
    true
}

/// Walk a codec's `AVCodecHWConfig` list looking for a matching device
/// type that uses the `hw_device_ctx` setup path (the only path our
/// `try_init_*` helpers wire up).
unsafe fn codec_supports_device(codec: *const sys::AVCodec, want: sys::AVHWDeviceType) -> bool {
    let mut i = 0;
    loop {
        let cfg = unsafe { sys::avcodec_get_hw_config(codec, i) };
        if cfg.is_null() {
            return false;
        }
        let methods = unsafe { (*cfg).methods };
        let device_type = unsafe { (*cfg).device_type };
        let ctx_method = sys::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as c_int;
        if device_type == want && (methods & ctx_method) != 0 {
            return true;
        }
        i += 1;
    }
}

/// `get_format` callback installed when VideoToolbox is active. Picks
/// `AV_PIX_FMT_VIDEOTOOLBOX` when ffmpeg offers it, else falls back to
/// whatever pixel format ffmpeg listed first (i.e. lets the codec run
/// in software for that frame).
unsafe extern "C" fn get_videotoolbox_format(
    _ctx: *mut sys::AVCodecContext,
    fmts: *const sys::AVPixelFormat,
) -> sys::AVPixelFormat {
    if fmts.is_null() {
        return sys::AVPixelFormat::AV_PIX_FMT_NONE;
    }
    let mut p = fmts;
    loop {
        let fmt = unsafe { *p };
        if fmt == sys::AVPixelFormat::AV_PIX_FMT_NONE {
            break;
        }
        if fmt == sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX {
            return sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
        }
        p = unsafe { p.offset(1) };
    }
    unsafe { *fmts }
}

pub(crate) fn decode_frame_at(
    ictx: &mut ff::format::context::Input,
    state: &mut VideoDecodeState,
    target_seconds: Rational64,
) -> Result<DecodedFrame> {
    let av_time_base = Rational64::from_integer(sys::AV_TIME_BASE as i64);
    let target_av = (target_seconds * av_time_base).to_integer().max(0);

    // Seek to (or before) the target in ffmpeg's wall-clock units.
    ictx.seek(target_av, ..target_av.saturating_add(1))?;
    state.decoder.flush();

    // Convert target into the stream's own tick rate so PTS comparisons
    // stay exact (esp. for fractional rates like 24000/1001).
    let target_ticks = if state.time_base.numer() == &0 {
        0
    } else {
        (target_seconds / state.time_base).to_integer()
    };

    let mut best: Option<ff::frame::Video> = None;
    let mut best_pts: i64 = i64::MIN;
    let mut overshot = false;

    'packets: for (stream, packet) in ictx.packets() {
        if stream.index() != state.stream_index {
            continue;
        }
        state.decoder.send_packet(&packet)?;
        loop {
            let mut decoded = ff::frame::Video::empty();
            match state.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    let frame = download_if_hw(state, decoded)?;
                    let pts = frame_pts(&frame);
                    if pts <= target_ticks {
                        if pts >= best_pts {
                            best = Some(frame);
                            best_pts = pts;
                        }
                    } else {
                        // First frame strictly past target — we have our
                        // answer. If `best` is None the seek overshot;
                        // use this frame instead of nothing.
                        if best.is_none() {
                            best = Some(frame);
                            best_pts = pts;
                        }
                        overshot = true;
                        break 'packets;
                    }
                }
                Err(ff::Error::Other { errno }) if errno == ff::error::EAGAIN => break,
                Err(ff::Error::Eof) => break 'packets,
                Err(e) => return Err(e.into()),
            }
        }
    }

    if !overshot {
        state.decoder.send_eof()?;
        loop {
            let mut decoded = ff::frame::Video::empty();
            match state.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    let frame = download_if_hw(state, decoded)?;
                    let pts = frame_pts(&frame);
                    let take = if pts <= target_ticks {
                        pts >= best_pts
                    } else {
                        best.is_none()
                    };
                    if take {
                        best = Some(frame);
                        best_pts = pts;
                    }
                }
                Err(_) => break,
            }
        }
    }

    let yuv = best.ok_or_else(|| MediaError::NoFrame {
        path: std::path::PathBuf::new(),
        target_seconds: 0.0,
    })?;
    let pts_seconds = state.time_base * Rational64::from_integer(best_pts);
    debug!(
        target = ?target_seconds,
        landed_ticks = best_pts,
        landed_seconds = ?pts_seconds,
        "decoded frame"
    );
    rgba_from_yuv(state, &yuv, pts_seconds)
}

/// If the decoded frame lives on a hardware surface, transfer it back
/// into a system-memory frame and copy props (PTS, etc.). Frames that
/// already live in CPU memory are returned untouched.
fn download_if_hw(state: &VideoDecodeState, frame: ff::frame::Video) -> Result<ff::frame::Video> {
    if state.hw_accel == HwAccel::None {
        return Ok(frame);
    }

    let raw_format = unsafe { (*frame.as_ptr()).format };
    let on_hw_surface = matches!(state.hw_accel, HwAccel::VideoToolbox)
        && raw_format == sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX as c_int;
    if !on_hw_surface {
        return Ok(frame);
    }

    let mut sw = ff::frame::Video::empty();
    // SAFETY: both pointers are valid AVFrames managed by ffmpeg-next.
    // `av_hwframe_transfer_data` allocates `sw`'s buffers and picks the
    // first acceptable software pixel format (NV12 for VideoToolbox).
    unsafe {
        let ret = sys::av_hwframe_transfer_data(sw.as_mut_ptr(), frame.as_ptr(), 0);
        if ret < 0 {
            return Err(ff::Error::from(ret).into());
        }
        // Transfer copies pixels only; props (pts, time_base, etc.)
        // need an explicit copy or downstream PTS comparisons break.
        let _ = sys::av_frame_copy_props(sw.as_mut_ptr(), frame.as_ptr());
    }
    Ok(sw)
}

fn frame_pts(frame: &ff::frame::Video) -> i64 {
    // `pts()` is the canonical presentation timestamp post-decode;
    // ffmpeg sorts B-frames into display order before handing them out.
    frame.pts().unwrap_or(0)
}

fn rgba_from_yuv(
    state: &mut VideoDecodeState,
    yuv: &ff::frame::Video,
    pts: Rational64,
) -> Result<DecodedFrame> {
    let in_fmt = yuv.format();

    // Build / rebuild the scaler when the input format changes. With
    // hwaccel we only learn the post-download format on the first
    // frame; with software decode it's stable from frame 1, so the
    // rebuild branch is a one-shot init in practice.
    let needs_new = !matches!(&state.scaler, Some((cached, _)) if *cached == in_fmt);
    if needs_new {
        let scaler = Scaler::get(
            in_fmt,
            state.width,
            state.height,
            Pixel::RGBA,
            state.width,
            state.height,
            Flags::BILINEAR,
        )?;
        state.scaler = Some((in_fmt, scaler));
    }
    let scaler = &mut state
        .scaler
        .as_mut()
        .expect("scaler initialized just above")
        .1;

    let mut rgba_frame = ff::frame::Video::empty();
    scaler.run(yuv, &mut rgba_frame)?;

    let w = state.width as usize;
    let h = state.height as usize;
    let stride = rgba_frame.stride(0);
    let data = rgba_frame.data(0);

    // swscale gives us stride-padded rows; collapse to tightly-packed
    // RGBA so downstream consumers (PNG encoders, wgpu uploads) don't
    // have to think about it.
    let mut rgba = Vec::with_capacity(w * h * 4);
    let row_bytes = w * 4;
    for y in 0..h {
        let start = y * stride;
        rgba.extend_from_slice(&data[start..start + row_bytes]);
    }

    Ok(DecodedFrame {
        width: state.width,
        height: state.height,
        rgba,
        pts,
    })
}
