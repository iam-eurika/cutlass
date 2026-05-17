# `engine`

Decode **scheduling** for Cutlass: one worker thread owns [`decoder::Decoder`] instances and answers “give me a frame at time *T*” via channels.

- Design: [`docs/engine/research.md`](../docs/engine/research.md)
- Plan / MVP phases: [`docs/engine/roadmap.md`](../docs/engine/roadmap.md)

## Quickstart

```rust
use engine::{Engine, EngineEvent, Rational};
use std::path::PathBuf;

let (engine, rx) = Engine::new();
let (sid, _open_rid) = engine.open(PathBuf::from("media.mp4"));

match rx.recv_timeout(std::time::Duration::from_secs(5)).expect("recv") {
    EngineEvent::Opened { .. } => {}
    other => panic!("{other:?}"),
}

let _ = engine.seek_exact(sid, Rational::new_raw(2, 1));
while let Ok(ev) = rx.recv() {
    println!("{ev:?}");
    break;
}
```

Run the smoke harness:

```bash
cargo run -p engine --example playground
cargo run -p engine --example playground -- /path/to/video.mp4
```

Optional script (lines like `seek_exact 2/1`, `next 5`, `scrub 5/2`, `close`, `sleep_ms 20`):

```bash
cargo run -p engine --example playground -- ./video.mp4 --script ./script.txt
```

## Contracts (MVP)

- **Drain events:** the outbound queue is bounded (`EVENT_CHANNEL_CAPACITY`); consumers must read or the worker blocks on send.
- **`seek_exact` vs `seek_scrub`:** exact seeks correlate with `RequestId` on events; scrub is latest-wins and uses `request_id: None` on frames.
- **One decoder:** opening a second source without closing the first yields `EngineError::SourceAlreadyOpen` on the event stream.

## Tests

- **`tests/engine_integration.rs`:** FFmpeg-backed worker tests (`cfg(unix)`): open/probe, demuxer errors, `seek_exact` / `next_frame` / EOF, scrub coalescing + ordering, single-decoder policy, `RequestId` correlation, cloned handles, and concurrent submits. Helpers in `tests/engine_integration/common.rs`. Fixtures under `tests/assets/` (symlinks into `crates/decoder/tests/assets/`).
