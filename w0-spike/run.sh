#!/usr/bin/env bash
# W0 spike harness. Usage (inside an X server, e.g. via xvfb-run): run.sh <mode> <renderer> <outdir> [name=value ...]
# Extra name=value arguments are passed to the spike (see src/main.rs). SPIKE_BIN overrides the binary dir.
# Starts the spike, injects X11 input at fixed times, records window state, waits for exit.
set -u
MODE=$1; R=$2; OUT=$3
BIN=${SPIKE_BIN:-/home/user/pdfcraft-w0/w0-spike/target/debug}
shift 3; EXTRA=("$@"); TAG=${TAG:-}
mkdir -p "$OUT"
[ -n "${WM:-}" ] && OUT="$OUT/wm" && mkdir -p "$OUT"
N="$MODE-$R$TAG"; H="$OUT/$N.harness.log"
: > "$H"
T0=$(date +%s.%N)
hl() { printf '[%8.3f] %s\n' "$(echo "$(date +%s.%N) - $T0" | bc)" "$*" >> "$H"; }
at() { # wait until $1 seconds after start
  while [ "$(echo "$(date +%s.%N) - $T0 < $1" | bc)" = 1 ]; do read -r -t 0.02 <> <(:) || true; done
}
win() { xdotool search --name "^$1\$" 2>/dev/null | head -1; }
mapstate() { xwininfo -id "$1" 2>/dev/null | grep -E 'Map State|Absolute upper-left|Width|Height' | tr -s ' ' | tr '\n' ' '; }
if [ -n "${WM:-}" ]; then openbox >"$OUT/openbox.log" 2>&1 & WMPID=$!; for _ in $(seq 1 50); do read -r -t 0.02 <> <(:) || true; done; hl "openbox started pid=$WMPID"; fi
SPIKE_MODE=$MODE SPIKE_RENDERER=$R SPIKE_LOG="$OUT/$N.log" "$BIN/w0-spike" "${EXTRA[@]}" 2> "$OUT/$N.stderr" &
PID=$!
hl "started pid=$PID mode=$MODE renderer=$R"
# The script clock starts at the spike's first root pass.
for _ in $(seq 1 600); do grep -q 'ROOT first pass' "$OUT/$N.log" 2>/dev/null && break; read -r -t 0.02 <> <(:) || true; done
T0=$(date +%s.%N); hl "T0 = first root pass seen"
case $MODE in
basic)
  at 1.6; A=$(win "W0 sec A"); B=$(win "W0 sec B"); ROOT=$(win "W0 root")
  hl "windows root=$ROOT A=$A B=$B"; for w in $ROOT $A $B; do hl "xwininfo $w: $(mapstate $w)"; done
  at 2.5; import -window root "$OUT/$N.screen.png"; hl "screen captured"
  at 5.5; hl "xclose A (dirty)"; "$BIN/xclose" "$A" 2>>"$H"
  at 6.5; hl "xclose A (clean)"; "$BIN/xclose" "$A" 2>>"$H"
  at 7.3; hl "remaining windows: $(xdotool search --name '^W0 ' 2>/dev/null | tr '\n' ' ')"
  ;;
h1)
  at 1.5; ROOT=$(win "W0 root"); S=$(win "W0 sec S"); hl "root=$ROOT S=$S"
  hl "root: $(mapstate $ROOT)"; hl "S: $(mapstate $S)"
  at 2.0; hl "xclose ROOT"; "$BIN/xclose" "$ROOT" 2>>"$H"
  at 3.2; hl "after H1: root: $(mapstate $ROOT)"; hl "S still exists? $(xwininfo -id $S >/dev/null 2>&1 && echo yes || echo no)"
  hl "windows named W0*: $(xdotool search --name '^W0 ' 2>/dev/null | tr '\n' ' ')"; hl "root title now: $(xdotool getwindowname $ROOT 2>/dev/null)"
  import -window root "$OUT/$N.screen.png"
  at 4.0; hl "xclose ROOT again (no other windows)"; "$BIN/xclose" "$ROOT" 2>>"$H"
  ;;
h2|h3)
  at 1.5; ROOT=$(win "W0 root"); S=$(win "W0 sec S"); hl "root=$ROOT S=$S"; hl "root: $(mapstate $ROOT)"; hl "S: $(mapstate $S)"
  if [ "$MODE" = h2 ]; then at 2.0; hl "xclose ROOT"; "$BIN/xclose" "$ROOT" 2>>"$H"; fi
  at 2.4; hl "root: $(mapstate $ROOT)"; hl "S: $(mapstate $S)"
  at 3.0; hl "click inside S"; xdotool mousemove 1000 300 click 1
  at 5.0; hl "xclose S (last window)"; "$BIN/xclose" "$S" 2>>"$H"
  ;;
solo)
  at 1.5; ROOT=$(win "W0 root"); hl "root=$ROOT"
  at 2.0; hl "xclose ROOT"; "$BIN/xclose" "$ROOT" 2>>"$H"
  ;;
panic)
  at 0.8; S=$(win "W0 sec S"); hl "S=$S"
  at 1.5; read -r CX CY < <(grep -o 'CLICK_TARGET root_button x=[0-9.-]* y=[0-9.-]*' "$OUT/$N.log" | head -1 | sed 's/.*x=\([0-9.-]*\) y=\([0-9.-]*\)/\1 \2/')
  hl "click root button at $CX,$CY"; [ -n "$CX" ] && xdotool mousemove "$CX" "$CY" click 1
  at 2.0; hl "xclose S (the window whose closure panicked)"; "$BIN/xclose" "$S" 2>>"$H"
  at 3.6; hl "windows: $(for w in $(xdotool search --name '^W0' 2>/dev/null); do printf '%s ' "$(xdotool getwindowname $w)"; done)"
  ;;
fail|embed|clamp|devlost|a11y)
  at 1.0; hl "windows: $(xdotool search --name '^W0' 2>/dev/null | tr '\n' ' ')"
  for w in $(xdotool search --name '^W0' 2>/dev/null); do hl "$w $(xdotool getwindowname $w): $(mapstate $w)"; done
  ;;
drag)
  # Drop-target classification. Coordinates assume openbox (WM=openbox): root inner at 44,63 px,
  # S inner at 868,128 px (Xvfb's 1.0833 scale); the 40 pt tab strip is the top 43 px of each.
  # 1 root strip -> S strip (valid), 2 root strip -> S page (invalid), 3 root strip -> desktop
  # (new window), 4 root strip -> desktop + Escape while held (cancel), 5 S strip -> root strip.
  at 1.0; ROOT=$(win "W0 root"); S=$(win "W0 sec S"); hl "root=$ROOT $(mapstate $ROOT)"; hl "S=$S $(mapstate $S)"
  drag() { # x0 y0 x1 y1 [escape]
    xdotool mousemove "$1" "$2"; nap 0.15; xdotool mousedown 1; nap 0.15
    xdotool mousemove $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 )); nap 0.15; xdotool mousemove "$3" "$4"; nap 0.2
    [ -n "${5:-}" ] && { hl "Escape while held"; xdotool key Escape; nap 0.2; }
    xdotool mouseup 1; hl "drag $1,$2 -> $3,$4 ${5:-} released at $(xdotool getmouselocation)"; nap 0.4
  }
  nap() { read -r -t "$1" <> <(:) || true; }
  drag 200 85 1000 150
  drag 200 85 1000 330
  drag 200 85 1500 900
  drag 200 85 1500 900 esc
  drag 1000 150 300 85
  ;;
esac
# wait for exit, at most 20 s
for _ in $(seq 1 ${WAIT_TICKS:-400}); do kill -0 $PID 2>/dev/null || break; read -r -t 0.05 <> <(:) || true; done
if kill -0 $PID 2>/dev/null; then hl "STILL RUNNING - killing"; kill $PID; wait $PID; RC=$?; hl "killed rc=$RC"; else wait $PID; RC=$?; hl "exited rc=$RC"; fi
OUTCOME=PASS; [ "$RC" != 0 ] && OUTCOME=FAIL
echo "[harness] RESULT check=${MODE}_exit renderer=$R outcome=$OUTCOME detail=rc=$RC ${EXTRA[*]}" >> "$OUT/$N.log"
[ -n "${WM:-}" ] && kill $WMPID 2>/dev/null
