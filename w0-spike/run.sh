#!/usr/bin/env bash
# W0 spike harness. Usage (inside an X server, e.g. via xvfb-run): run.sh <mode> <renderer> <outdir>
# Starts the spike, injects X11 input at fixed times, records window state, waits for exit.
set -u
MODE=$1; R=$2; OUT=$3
BIN=/home/user/pdfcraft-w0/w0-spike/target/debug
mkdir -p "$OUT"
H="$OUT/$MODE-$R.harness.log"
[ -n "${WM:-}" ] && OUT="$OUT/wm" && mkdir -p "$OUT" && H="$OUT/$MODE-$R.harness.log"
: > "$H"
T0=$(date +%s.%N)
hl() { printf '[%8.3f] %s\n' "$(echo "$(date +%s.%N) - $T0" | bc)" "$*" >> "$H"; }
at() { # wait until $1 seconds after start
  while [ "$(echo "$(date +%s.%N) - $T0 < $1" | bc)" = 1 ]; do read -r -t 0.02 <> <(:) || true; done
}
win() { xdotool search --name "^$1\$" 2>/dev/null | head -1; }
mapstate() { xwininfo -id "$1" 2>/dev/null | grep -E 'Map State|Absolute upper-left|Width|Height' | tr -s ' ' | tr '\n' ' '; }
if [ -n "${WM:-}" ]; then openbox >"$OUT/openbox.log" 2>&1 & WMPID=$!; for _ in $(seq 1 50); do read -r -t 0.02 <> <(:) || true; done; hl "openbox started pid=$WMPID"; fi
SPIKE_MODE=$MODE SPIKE_RENDERER=$R SPIKE_LOG="$OUT/$MODE-$R.log" "$BIN/w0-spike" 2> "$OUT/$MODE-$R.stderr" &
PID=$!
hl "started pid=$PID mode=$MODE renderer=$R"
# The script clock starts at the spike's first root pass.
for _ in $(seq 1 600); do grep -q 'ROOT first pass' "$OUT/$MODE-$R.log" 2>/dev/null && break; read -r -t 0.02 <> <(:) || true; done
T0=$(date +%s.%N); hl "T0 = first root pass seen"
case $MODE in
basic)
  at 1.6; A=$(win "W0 sec A"); B=$(win "W0 sec B"); ROOT=$(win "W0 root")
  hl "windows root=$ROOT A=$A B=$B"; for w in $ROOT $A $B; do hl "xwininfo $w: $(mapstate $w)"; done
  at 2.5; import -window root "$OUT/$MODE-$R.screen.png"; hl "screen captured"
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
  import -window root "$OUT/$MODE-$R.screen.png"
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
fail|embed|panic)
  at 1.0; hl "windows: $(xdotool search --name '^W0' 2>/dev/null | tr '\n' ' ')"
  for w in $(xdotool search --name '^W0' 2>/dev/null); do hl "$w $(xdotool getwindowname $w): $(mapstate $w)"; done
  ;;
drag)
  at 1.0; ROOT=$(win "W0 root"); S=$(win "W0 sec S"); hl "root=$ROOT $(mapstate $ROOT)"; hl "S=$S $(mapstate $S)"
  step() { xdotool mousemove "$1" "$2"; hl "xdotool at $(xdotool getmouselocation)"; at "$3"; }
  at 1.5; step 200 200 2.0
  hl "mousedown 1 (inside root)"; xdotool mousedown 1; at 2.5
  step 600 300 3.0; step 760 300 3.5; step 900 200 4.0; step 1300 900 4.5; step 1900 1070 5.0
  hl "key Escape while held outside"; xdotool key Escape; at 5.5
  hl "mouseup 1 at desktop (outside every window)"; xdotool mouseup 1; at 6.0
  step 950 250 6.5; hl "mousedown in S"; xdotool mousedown 1; at 7.0
  step 300 300 7.5; hl "mouseup over ROOT"; xdotool mouseup 1; at 8.0
  step 300 300 8.3; hl "key Escape over root, no button"; xdotool key Escape; at 9.0
  ;;
esac
# wait for exit, at most 20 s
for _ in $(seq 1 400); do kill -0 $PID 2>/dev/null || break; read -r -t 0.05 <> <(:) || true; done
if kill -0 $PID 2>/dev/null; then hl "STILL RUNNING - killing"; kill $PID; wait $PID; hl "killed rc=$?"; else wait $PID; hl "exited rc=$?"; fi
[ -n "${WM:-}" ] && kill $WMPID 2>/dev/null
