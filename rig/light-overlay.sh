# tachys light-overlay recipe, a hook for the peeryard rig (sourced by rig.sh inside its namespace).
# Can one wallet learn the chain tip from another wallet, with no full node in its path?
#   A  = the v6.0.8 release, mining
#   T1 = a namespace holding a tachys that syncs from A and then serves what it verified
#   T2 = a namespace linked ONLY to T1, holding a tachys that syncs from T1
# T1 and T2 are declared as deferred nodes so the rig builds their namespaces and links; no node is
# ever launched in them.
# Run:  SCRATCH=<dir> TACHYS_ROOT=<tachys checkout> TACHYS_JAR_RELEASE=<jar> \
#         bash <peeryard>/rig/rig.sh $TACHYS_ROOT/rig/light-overlay.json $TACHYS_ROOT/rig/light-overlay.sh
T="$TACHYS_ROOT/target/release/tachys"; W="$SCRATCH/tachys"; mkdir -p "$W"; NETF="$W/net.json"
in1() { ip netns exec "${NS[T1]}" "$T" "$@" --net-file "$NETF"; }
in2() { ip netns exec "${NS[T2]}" "$T" "$@" --net-file "$NETF"; }
fail() { echo "[overlay] FAIL: $*"; rig_verdict=FAIL; }
rig_verdict=PASS
A_FOR_T1="${LIP["A,${LINK_OF["A,T1"]}"]}"; T1_FOR_T2="${LIP["T1,${LINK_OF["T1,T2"]}"]}"

end=$((SECONDS+240)); while [[ $SECONDS -lt $end ]]; do [[ "$(full_height A)" -ge 20 ]] && break; sleep 4; done
gen="$(header_at A 1)"; cpid="$(header_at A $(( $(full_height A) - 12 )))"
rest A "/blocks/$cpid/header" > "$W/cp.json"
jq -n --arg g "$gen" --argjson magic "$MAGIC" --slurpfile cp "$W/cp.json" \
  '{name:"devnet", magic:$magic, genesis:$g, checkpoint:$cp[0], floor_divisor:1048576, v2_activation_height:0, block_interval_ms:3000, seeds:""}' > "$NETF"
echo "$A_FOR_T1:${P2P[A]}" > "$W/seeds-a.txt"; echo "$T1_FOR_T2:9030" > "$W/seeds-t1.txt"
echo "[overlay] A at height $(full_height A); A is $A_FOR_T1 to T1; T1 is $T1_FOR_T2 to T2"

echo "[overlay] 0. control: T2 has no route to the full node"
c0="$(in2 tips --seeds "$W/seeds-a.txt" --peers 1 --tries 1 --quorum 1 --state "$W/t2-direct" 2>&1 | tail -n 3)"; echo "$c0" | sed 's/^/  /'
echo "$c0" | grep -q "^verdict: Agreed" && fail "T2 reached the full node directly; the topology does not isolate it"

echo "[overlay] 1. T1 syncs from the full node"
o1="$(in1 tips --seeds "$W/seeds-a.txt" --peers 1 --tries 1 --quorum 1 --state "$W/t1")"; echo "$o1" | tail -n 4 | sed 's/^/  /'
echo "$o1" | grep -q "^verdict: Agreed" || fail "T1 tip sync"
tip1="$(echo "$o1" | sed -n 's/^tip: height \([0-9]*\) id \([0-9a-f]*\).*/\1 \2/p')"

echo "[overlay] 2. T1 serves; T2 syncs from T1 only"
in1 serve --state "$W/t1" --listen "$T1_FOR_T2:9030" --secs 12 > "$W/serve.log" 2>&1 &
SERVE=$!; sleep 2
o2="$(in2 tips --seeds "$W/seeds-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/t2")"; echo "$o2" | tail -n 6 | sed 's/^/  /'
tip2="$(echo "$o2" | sed -n 's/^tip: height \([0-9]*\) id \([0-9a-f]*\).*/\1 \2/p')"
echo "$o2" | grep -q "^verdict: Agreed" || fail "T2 did not reach a verdict from T1"
[[ -n "$tip1" && "$tip1" == "$tip2" ]] && echo "  T2's tip equals T1's: $tip2" || fail "tips differ: T1 '$tip1' T2 '$tip2'"
echo "  and the full node has that header at that height: $([[ "$(header_at A "${tip2%% *}")" == "${tip2##* }" ]] && echo yes || { echo NO; rig_verdict=FAIL; })"
wait "$SERVE"; sed 's/^/  T1: /' "$W/serve.log"

echo "[overlay] 3. control: T1 serves a chain with one broken header; T2 must refuse it"
in1 serve --state "$W/t1" --listen "$T1_FOR_T2:9030" --secs 12 --tamper-for-control > "$W/serve-bad.log" 2>&1 &
SERVE=$!; sleep 2
o3="$(in2 tips --seeds "$W/seeds-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/t2-bad" 2>&1)"; echo "$o3" | tail -n 5 | sed 's/^/  /'
echo "$o3" | grep -q "^verdict: Agreed" && fail "T2 accepted a chain with a header whose work is broken"
wait "$SERVE"; sed 's/^/  T1: /' "$W/serve-bad.log"
echo "[overlay] 4. a serving wallet that keeps syncing serves a tip that moves"
ip netns exec "${NS[T1]}" "$T" serve --state "$W/t1" --listen "$T1_FOR_T2:9030" --secs 40 --follow-state --net-file "$NETF" > "$W/serve-follow.log" 2>&1 &
SERVE=$!; sleep 2
h_a="$(in2 tips --seeds "$W/seeds-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/t2-follow" | sed -n 's/^tip: height \([0-9]*\).*/\1/p')"
for i in 1 2 3; do sleep 5; in1 tips --seeds "$W/seeds-a.txt" --peers 1 --tries 1 --quorum 1 --state "$W/t1" > /dev/null; done
sleep 4
h_b="$(in2 tips --seeds "$W/seeds-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/t2-follow" | sed -n 's/^tip: height \([0-9]*\).*/\1/p')"
echo "  T2 saw height $h_a from T1, then height $h_b after T1 had synced three more times (node is at $(full_height A))"
[[ -n "$h_a" && -n "$h_b" && "$h_b" -gt "$h_a" ]] || fail "the served tip did not advance"
kill "$SERVE" 2>/dev/null; wait "$SERVE" 2>/dev/null
echo "[overlay] verdict: $rig_verdict"
