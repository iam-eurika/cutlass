//! Single background thread: latest-wins seeks, scrub throttle, debounced exact.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use crate::preview::{PreviewRender, PreviewSeek, PreviewSession};
use crate::ui;

/// Minimum interval between scrub preview updates (~30 Hz).
pub const SCRUB_MIN_INTERVAL: Duration = Duration::from_millis(33);

/// After the last scrub, wait this long then run an exact seek.
pub const EXACT_SEEK_DEBOUNCE: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy)]
struct PendingSeek {
    seconds: f32,
    mode: PreviewSeek,
}

#[derive(Debug, Clone, Copy)]
struct DebouncedExact {
    seconds: f32,
    deadline: Instant,
}

/// Coalesced preview requests (one worker thread).
#[derive(Clone)]
pub struct PreviewWorker {
    slot: Arc<Mutex<Option<PendingSeek>>>,
    debounce: Arc<Mutex<Option<DebouncedExact>>>,
    wake: Sender<()>,
}

impl PreviewWorker {
    pub fn spawn(
        session: Arc<Mutex<PreviewSession>>,
        ui_handle: slint::Weak<ui::PreviewWindow>,
    ) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let debounce = Arc::new(Mutex::new(None));
        let (wake, rx_wake) = bounded(1);

        let slot_t = Arc::clone(&slot);
        let debounce_t = Arc::clone(&debounce);
        let wake_t = wake.clone();
        thread::spawn(move || {
            worker_loop(session, ui_handle, slot_t, debounce_t, rx_wake, wake_t);
        });

        Self {
            slot,
            debounce,
            wake,
        }
    }

    /// Queue a seek (latest-wins; overwrites any pending request in the slot).
    pub fn request(&self, seconds: f32, mode: PreviewSeek) {
        *self.slot.lock().expect("slot") = Some(PendingSeek { seconds, mode });
        if mode == PreviewSeek::Exact {
            *self.debounce.lock().expect("debounce") = None;
        }
        let _ = self.wake.try_send(());
    }

    /// While scrubbing: fast keyframe preview plus debounced exact seek on release.
    pub fn request_scrub(&self, seconds: f32) {
        *self.slot.lock().expect("slot") = Some(PendingSeek {
            seconds,
            mode: PreviewSeek::Scrub,
        });
        *self.debounce.lock().expect("debounce") = Some(DebouncedExact {
            seconds,
            deadline: Instant::now() + EXACT_SEEK_DEBOUNCE,
        });
        let _ = self.wake.try_send(());
    }
}

fn worker_loop(
    session: Arc<Mutex<PreviewSession>>,
    ui_handle: slint::Weak<ui::PreviewWindow>,
    slot: Arc<Mutex<Option<PendingSeek>>>,
    debounce: Arc<Mutex<Option<DebouncedExact>>>,
    rx_wake: Receiver<()>,
    wake: Sender<()>,
) {
    let mut last_scrub_finished = Instant::now() - SCRUB_MIN_INTERVAL;

    loop {
        let sleep_for = debounce
            .lock()
            .expect("debounce")
            .map(|d| d.deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(100))
            .min(Duration::from_millis(100));

        if rx_wake.recv_timeout(sleep_for).is_ok() {
            while rx_wake.try_recv().is_ok() {}
        }

        if let Some(d) = *debounce.lock().expect("debounce") {
            if Instant::now() >= d.deadline {
                *debounce.lock().expect("debounce") = None;
                run_preview(
                    &session,
                    &ui_handle,
                    d.seconds,
                    PreviewSeek::Exact,
                    true,
                );
            }
        }

        let Some(mut job) = slot.lock().expect("slot").take() else {
            continue;
        };

        while let Some(newer) = slot.lock().expect("slot").take() {
            job = newer;
        }

        if job.mode == PreviewSeek::Scrub {
            let since_last = Instant::now().saturating_duration_since(last_scrub_finished);
            if since_last < SCRUB_MIN_INTERVAL {
                *slot.lock().expect("slot") = Some(job);
                thread::sleep(SCRUB_MIN_INTERVAL - since_last);
                let _ = wake.try_send(());
                continue;
            }
            last_scrub_finished = Instant::now();
        }

        run_preview(
            &session,
            &ui_handle,
            job.seconds,
            job.mode,
            job.mode == PreviewSeek::Exact,
        );
    }
}

fn run_preview(
    session: &Arc<Mutex<PreviewSession>>,
    ui_handle: &slint::Weak<ui::PreviewWindow>,
    seconds: f32,
    mode: PreviewSeek,
    refresh_range: bool,
) {
    let (result, rgba) = {
        let mut s = session.lock().expect("session");
        let timeline = crate::seconds_to_rational(seconds);
        match s.preview_render(timeline, mode) {
            Ok(PreviewRender::Gap) => (Ok(PreviewRender::Gap), Vec::new()),
            Ok(frame @ PreviewRender::Frame { .. }) => {
                let rgba = s.rgba_buf.clone();
                (Ok(frame), rgba)
            }
            Err(e) => (Err(e), Vec::new()),
        }
    };

    let ui_handle = ui_handle.clone();
    let session = Arc::clone(session);
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = ui_handle.upgrade() else {
            return;
        };
        match result {
            Ok(render) => {
                ui::apply_render_to_window(&ui, render, &rgba);
                if refresh_range {
                    let s = session.lock().expect("session");
                    ui::refresh_playhead_range(&s, &ui);
                    let max = ui::effective_playhead_max_seconds(&s);
                    ui.set_playhead_seconds(seconds.min(max));
                }
            }
            Err(e) => ui.set_status_text(format!("Preview error: {e}").into()),
        }
    });
}
