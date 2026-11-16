#!/usr/bin/env bash
# Three-arm experiment: what a freshly syncing node B asks of a watching client, by declared mode.
# Needs node A already running (rig/devnet.sh) and rig/out/devnet-net.json. Writes to $1.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; OUT="$1"; RUNTIME="$2"; JAR="$ROOT/rig/out/ergo-6.0.8-boxproof.jar"
: > "$OUT"
for arm in nothing light full-claim-control; do
  D="$ROOT/rig/out/devnetB"; rm -rf "$D"
  ( cd "$RUNTIME" && BIND_IP=127.0.0.2 P2P_PORT=19031 REST_PORT=19062 KNOWN_PEERS='["127.0.0.1:19021"]' MINING=false EXTRA_CONF="${EXTRA_CONF:-}" \
      exec "$ROOT/rig/devnet.sh" "$JAR" "$D" 6s > "$ROOT/rig/out/nodeB-$arm.log" 2>&1 ) &
  PIDB=$!
  echo "== arm: $arm" >> "$OUT"
  "$ROOT/target/release/tachys" observe --net-file "$ROOT/rig/out/devnet-net.json" --declare "$arm" --peer 127.0.0.2:19031 --secs 30 >> "$OUT" 2>&1
  echo "B after the arm: $(curl -s -m 3 http://127.0.0.2:19062/info | jq -c '{fullHeight,headersHeight,peersCount}')  A: $(curl -s -m 3 http://127.0.0.1:19052/info | jq -c '{fullHeight}')" >> "$OUT"
  kill "$PIDB" 2>/dev/null; for i in $(seq 1 20); do kill -0 "$PIDB" 2>/dev/null || break; sleep 1; done
  kill -0 "$PIDB" 2>/dev/null && echo "WARNING: node B (pid $PIDB) did not stop" >> "$OUT"
done
echo "finished" >> "$OUT"
