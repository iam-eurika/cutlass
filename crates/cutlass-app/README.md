# cutlass-app

`cutlass-app` is a small end-to-end CLI for exercising the Cutlass engine without launching the desktop UI. It imports sample media, builds a short project, renders a preview frame, saves a `.cutlass` project file, and exports an MP4.

This crate is useful as a smoke test for the full editing pipeline.

## What It Does

When run from the repository root, the app:

- Looks for MP4 files in `local-assets/assets/`.
- Picks up to three media files for a demo session.
- Creates a new engine session.
- Imports the selected media.
- Adds a video track and places clips in sequence.
- Requests one preview frame.
- Saves the project to `.cutlass/projects/<name>.cutlass`.
- Exports the result to `.cutlass/exports/<name>.mp4`.
- Uses `.cutlass/cache/` for preview cache data.

## Usage

Run the default demo:

```bash
cargo run -p cutlass-app
```

Choose a session name:

```bash
cargo run -p cutlass-app -- --name demo_edit
```

You can also set the name with `CUTLASS_NAME`:

```bash
CUTLASS_NAME=demo_edit cargo run -p cutlass-app
```

The app expects at least one `.mp4` file in `local-assets/assets/`.

## Scope

`cutlass-app` is not the desktop editor and is not meant to be a full command-line editing interface. Use `cutlass-ui` for interactive editing.

Keep this crate focused on simple end-to-end coverage. New engine features should usually be tested in `cutlass-engine`; this CLI should only grow when it helps exercise the full import-preview-save-export path.

## Testing

Run the crate with local media:

```bash
cargo run -p cutlass-app
```

There are no standalone unit tests in this crate today. Its value is in driving real workspace integration.
