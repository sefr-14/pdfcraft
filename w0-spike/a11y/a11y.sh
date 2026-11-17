#!/usr/bin/env bash
# W0 a11y probe on Linux: a private D-Bus session with the AT-SPI bus, accessibility enabled,
# the spike in mode=a11y (or another mode), then a dump of what a screen reader can see.
# Usage: xvfb-run -a a11y/a11y.sh <wgpu|glow> <outdir> [name=value ...]   (SPIKE_BIN overrides the binary dir)
# Needs: at-spi2-core, python3-pyatspi (system python3), dbus.
set -u
R=$1; OUT=$2; shift 2; EXTRA=("$@")
BIN=${SPIKE_BIN:-/home/user/pdfcraft-w0/w0-spike/target/debug}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"; NAME="a11y-$R${TAG:-}"
exec dbus-run-session -- bash -c '
  nap() { read -r -t "$1" <> <(:) || true; }
  /usr/libexec/at-spi-bus-launcher --launch-immediately > "$0/'"$NAME"'.bus.log" 2>&1 &
  nap 1
  gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled "<true>" >/dev/null
  echo "a11y IsEnabled: $(gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.freedesktop.DBus.Properties.Get org.a11y.Status IsEnabled)"
  "$1/w0-spike" renderer="$2" log="$0/'"$NAME"'.log" "${@:4}" 2> "$0/'"$NAME"'.stderr" &
  PID=$!
  nap "${DUMP_AT:-4}"
  ${PY:-/usr/bin/python3.12} "$3/atspi_dump.py" w0 6 > "$0/'"$NAME"'.tree.txt" 2>&1
  nap 1
  ${PY:-/usr/bin/python3.12} "$3/atspi_dump.py" w0 6 > "$0/'"$NAME"'.tree2.txt" 2>&1
  wait $PID; echo "spike rc=$?"
' "$OUT" "$BIN" "$R" "$HERE" "${EXTRA[@]}"
