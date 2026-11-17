#!/usr/bin/env bash
# W0-9 harness: send N WM_DELETE_WINDOW messages to one window and count them at each layer.
# Usage (inside an X server): closes.sh <wgpu|glow> <root|sec> <N> <interval_ms> <outdir> [name=value ...]
# Env: WM=openbox starts a window manager; TAG names the run. Prints one CLOSES summary line.
# Layers: "accepted" = the X server accepted the SendEvent (xclose checks the reply);
# "eframe" = eframe logged Received WindowEvent::CloseRequested for that viewport;
# "app" = the spike saw close_requested() in that viewport's pass (CLOSE_RX), every close held.
set -u
R=$1; TARGET=$2; N=$3; IVL=$4; OUT=$5; shift 5; EXTRA=("$@")
BIN=${SPIKE_BIN:-/home/user/pdfcraft-w0/w0-spike/target/debug}
mkdir -p "$OUT"; NAME="closes-$R-$TARGET${TAG:-}"; LOG="$OUT/$NAME.log"; H="$OUT/$NAME.harness.log"; : > "$H"
nap() { read -r -t "$1" <> <(:) || true; }
if [ -n "${WM:-}" ]; then openbox >"$OUT/$NAME.openbox.log" 2>&1 & WMPID=$!; nap 1; fi
SECS=$(echo "4 + $N * ($IVL + ${PAD_MS:-0}) / 1000 + 3" | bc)   # PAD_MS: extra time each send takes
"$BIN/w0-spike" mode=closes renderer="$R" log="$LOG" secs="$SECS" "${EXTRA[@]}" 2> "$OUT/$NAME.stderr" &
PID=$!
for _ in $(seq 1 600); do grep -q 'CLOSES ready' "$LOG" 2>/dev/null && break; nap 0.02; done
nap 1
if [ "$TARGET" = root ]; then TITLE="W0 root"; else TITLE="W0 sec C1"; fi
WIN=$(xdotool search --name "^$TITLE\$" 2>/dev/null | head -1)
VID=FFFF; [ "$TARGET" = sec ] && VID=$(grep -o 'Creating new viewport "[0-9A-F]*" (Some("W0 sec C1"))' "$LOG" | head -1 | cut -d'"' -f2)
echo "target=$TITLE win=$WIN viewport=$VID" >> "$H"
ACC=0
for i in $(seq 1 "$N"); do
  if "$BIN/xclose" "$WIN" 2>>"$H"; then ACC=$((ACC+1)); fi
  echo "sent $i at $(date +%s.%N)" >> "$H"
  nap "$(echo "$IVL / 1000" | bc -l)"
done
nap 1.5
EF=$(grep -c "Received WindowEvent::CloseRequested for viewport Some(\"$VID\")" "$LOG")
APP=$(grep -c "CLOSE_RX who=$TITLE " "$LOG")
for _ in $(seq 1 400); do kill -0 $PID 2>/dev/null || break; nap 0.05; done
kill -0 $PID 2>/dev/null && kill $PID; wait $PID; RC=$?
[ -n "${WM:-}" ] && kill $WMPID 2>/dev/null
LINE="CLOSES renderer=$R target=$TARGET wm=${WM:-none} N=$N interval_ms=$IVL accepted=$ACC eframe=$EF app=$APP rc=$RC extra=${EXTRA[*]}"
echo "$LINE" | tee -a "$H"
OUTCOME=PASS; [ "$APP" != "$N" ] && OUTCOME=FAIL
echo "[harness] RESULT check=closes_${TARGET} renderer=$R outcome=$OUTCOME detail=sent=$N accepted=$ACC eframe=$EF app=$APP wm=${WM:-none} ${EXTRA[*]}" >> "$LOG"
