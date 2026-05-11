#!/usr/bin/env bash
# Build the small mp4 fixtures used by the seek-precision integration tests.
#
# Each fixture is a tiny labeled clip (320x180, no audio) generated with
# ffmpeg's `lavfi` testsrc. They are intentionally NOT committed: the
# integration tests invoke this script (idempotently) before running.
#
# Idempotent: existing fixtures are left alone, so iterating on a single
# fixture means deleting just that file.
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "build.sh: ffmpeg not found in PATH" >&2
  exit 127
fi

build() {
  local out="$1"
  shift
  if [[ -f "$out" ]]; then
    echo "skip $out (exists)"
    return 0
  fi
  echo "building $out"
  ffmpeg -hide_banner -loglevel error -y "$@" "$out"
}

# Common knobs:
#   - 320x180, yuv420p, no audio, libx264.
#   - testsrc renders a moving label so visually-inspecting a decoded
#     frame is still useful when debugging.

# 30/1 fps CFR. PTS land on integer multiples of 1/30 s.
build cfr_30.mp4 \
  -f lavfi -i "testsrc=duration=2:size=320x180:rate=30" \
  -c:v libx264 -pix_fmt yuv420p -an -r 30 \
  -movflags +faststart

# 30000/1001 fps NTSC. Forced timescale 60000 so frame N's PTS is an
# exact integer multiple of the stream time_base — any float-based PTS
# math elsewhere shows up as a Rational64 inequality in the test.
# 4 s gives ~120 frames, enough for the frame-100 assertion.
build cfr_2997.mp4 \
  -f lavfi -i "testsrc=duration=4:size=320x180:rate=30000/1001" \
  -c:v libx264 -pix_fmt yuv420p -an -r 30000/1001 \
  -video_track_timescale 60000 \
  -movflags +faststart

# 30 fps with B-frames so the decoder must reorder packet (decode) order
# back into display order before we observe pts_seconds.
build bframes_30.mp4 \
  -f lavfi -i "testsrc=duration=2:size=320x180:rate=30" \
  -c:v libx264 -pix_fmt yuv420p -an -r 30 \
  -bf 3 -g 60 \
  -movflags +faststart

# 50 fps with the container's video time_base forced to 1/50 — matches
# an unusual real asset shape we have, and exercises the seek code
# against a small time_base denominator.
build cfr_50_tb50.mp4 \
  -f lavfi -i "testsrc=duration=2:size=320x180:rate=50" \
  -c:v libx264 -pix_fmt yuv420p -an -r 50 \
  -video_track_timescale 50 \
  -movflags +faststart
