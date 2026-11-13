#!/bin/sh
# Replays a recorded yt-dlp result. The last argument is the target; -f selects
# the format, -o names the download destination. Metadata goes to stdout; a
# download request is answered by copying the committed FLAC payload.
while [ "$#" -gt 0 ]; do
  case "$1" in
    -f) FORMAT="$2"; shift 2 ;;
    -o) OUT="$2"; shift 2 ;;
    *) TARGET="$1"; shift ;;
  esac
done
if [ -n "${OUT:-}" ]; then
  cp "$(dirname "$0")/../probe/sine_44100_s16.flac" "$OUT"
  exit 0
fi
if [ "$TARGET" = "scsearch10:nightwave neon" ]; then
  cat "$(dirname "$0")/yt-dlp-search.json"
else
  cat "$(dirname "$0")/yt-dlp-url.json"
fi
