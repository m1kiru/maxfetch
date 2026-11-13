#!/bin/sh
# Delivers a real lossy file: an MP3 192 kbps, never a FLAC.
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) OUT="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cp "$(dirname "$0")/../probe/sine_128k.mp3" "$OUT"
