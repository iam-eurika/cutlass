mod ruler;
mod timecode;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;

    // Install the ruler tick generator. Slint will invoke this whenever
    // any of the dependent properties (scroll-x, viewport width, zoom,
    // fps, drop-frame) change — see `ui/lib/ruler-backend.slint` for
    // the contract and `ui/panels/timeline/ruler.slint` for the call site.
    app.global::<RulerBackend>().on_ticks(
        |scroll_x, viewport_w, zoom, fps_num, fps_den, drop_frame| {
            ruler::ticks_model(scroll_x, viewport_w, zoom, fps_num, fps_den, drop_frame)
        },
    );

    app.run()
}
