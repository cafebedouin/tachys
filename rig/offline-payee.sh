# tachys offline-payee recipe, a hook for the peeryard rig (sourced by rig.sh inside its namespace).
# A payee with no route to any node checks a receipt through another wallet, which passes the
# request to its node and the answer back. The payee verifies everything itself.
#   A  = v6.0.8 + node-patches/0001, mining      T2 = payer wallet (reaches A)
#   T1 = relaying wallet (reaches A and T3)      T3 = payee wallet (reaches T1 only)
# Run:  SCRATCH=<dir> TACHYS_ROOT=<tachys checkout> TACHYS_JAR_PATCHED=<jar> \
#         bash <peeryard>/rig/rig.sh $TACHYS_ROOT/rig/offline-payee.json $TACHYS_ROOT/rig/offline-payee.sh
T="$TACHYS_ROOT/target/release/tachys"; W="$SCRATCH/tachys"; mkdir -p "$W"; NETF="$W/net.json"
in1() { ip netns exec "${NS[T1]}" "$T" "$@" --net-file "$NETF"; }
in2() { ip netns exec "${NS[T2]}" "$T" "$@" --net-file "$NETF"; }
in3() { ip netns exec "${NS[T3]}" "$T" "$@" --net-file "$NETF"; }
fail() { echo "[op] FAIL: $*"; rig_verdict=FAIL; }
rig_verdict=PASS
A1="${LIP["A,${LINK_OF["A,T1"]}"]}:${P2P[A]}"; A2="${LIP["A,${LINK_OF["A,T2"]}"]}:${P2P[A]}"; T1ADDR="${LIP["T1,${LINK_OF["T1,T3"]}"]}:9030"

end=$((SECONDS+240)); while [[ $SECONDS -lt $end ]]; do [[ "$(full_height A)" -ge 20 ]] && break; sleep 4; done
gen="$(header_at A 1)"; cpid="$(header_at A $(( $(full_height A) - 12 )))"; rest A "/blocks/$cpid/header" > "$W/cp.json"
jq -n --arg g "$gen" --argjson magic "$MAGIC" --slurpfile cp "$W/cp.json" \
  '{name:"devnet", magic:$magic, genesis:$g, checkpoint:$cp[0], floor_divisor:1048576, v2_activation_height:0, block_interval_ms:3000, seeds:""}' > "$NETF"
echo "$A1" > "$W/seeds-a1.txt"; echo "$A2" > "$W/seeds-a2.txt"; echo "$T1ADDR" > "$W/seeds-t1.txt"

echo "[op] 1. the payer wallet is funded and proves its box (online, from the node)"
in2 tips --seeds "$W/seeds-a2.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payer" > /dev/null
paddr="$(in2 wallet new --state "$W/payer" | sed -n 's/^new wallet: address //p')"
wait_balance A 2000000000 240 >/dev/null
txid="$(wallet A /wallet/payment/send "[{\"address\":\"$paddr\",\"value\":500000000}]" | tr -d '"')"
box=""; end=$((SECONDS+120)); while [[ $SECONDS -lt $end && -z "$box" ]]; do sleep 6
  box="$(wallet A "/wallet/transactionById?id=$txid" | jq -r --arg a "$paddr" 'select(.numConfirmations != null and .numConfirmations >= 1) | .outputs[] | select(.address==$a) | .boxId' 2>/dev/null | head -1)"; done
feetree="$(wallet A "/wallet/transactionById?id=$txid" | jq -r '.outputs[] | select(.value==1000000) | .ergoTree' | head -1)"
jq --arg f "$feetree" '. + {fee_tree:$f}' "$NETF" > "$NETF.tmp" && mv "$NETF.tmp" "$NETF"
in2 tips --seeds "$W/seeds-a2.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payer" > /dev/null
in2 wallet receive --state "$W/payer" --node "p2p://$A2" --ids "$box" | tail -n 2 | head -n 1 | sed 's/^/  payer: /'

echo "[op] 2. the relaying wallet syncs and serves, with proof relay on"
in1 tips --seeds "$W/seeds-a1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/relay" > /dev/null
ip netns exec "${NS[T1]}" "$T" serve --state "$W/relay" --listen "$T1ADDR" --secs 150 --relay-proofs "$A1" --net-file "$NETF" > "$W/serve.log" 2>&1 &
SERVE=$!; sleep 2

echo "[op] 3. the payee has no route to the node; it syncs from the relaying wallet and makes an address"
c="$(in3 tips --seeds "$W/seeds-a1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/probe" 2>&1 | tail -n 1)"; echo "  payee -> node directly: $c"
echo "$c" | grep -q "^state root" && fail "the payee reaches the node"
in3 tips --seeds "$W/seeds-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payee" | grep "^verdict\|^tip" | sed 's/^/  /'
qaddr="$(in3 wallet new --state "$W/payee" | sed -n 's/^new wallet: address //p')"

echo "[op] 4. the payer pays the payee 0.1 ERG, submitting over the peer protocol, and gives it the receipt"
mkdir -p "$W/payer/outbox"
o="$(in2 wallet send --state "$W/payer" --to "$qaddr" --nanoerg 100000000 --parcel-out "$W/payer/outbox/own.json")"
receipt="$(echo "$o" | sed -n 's/^receipt for the payee (box ids to hand over): //p')"; echo "  receipt: $receipt"
in2 tips --seeds "$W/seeds-a2.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payer" > /dev/null
in2 outbox --state "$W/payer" --peer "$A2" | tail -n 2 | sed 's/^/  /'
end=$((SECONDS+90)); onchain=""; while [[ $SECONDS -lt $end && -z "$onchain" ]]; do sleep 5
  [[ "$(ip netns exec "${NS[A]}" curl -s -o /dev/null -w '%{http_code}' --max-time 4 "http://127.0.0.1:${REST[A]}/utxo/byId/$receipt")" == 200 ]] && onchain=yes; done
[[ -n "$onchain" ]] && echo "  the receipt box is on chain" || fail "payment never confirmed"

echo "[op] 5. the payee checks the receipt through the relaying wallet"
o="$(in3 wallet receive --state "$W/payee" --node "p2p://$T1ADDR" --ids "$receipt")"; echo "$o" | sed 's/^/  /'
echo "$o" | grep -q "(added)" || fail "the payee could not verify its receipt through the relay"
echo "$o" | grep -q "balance 0.100000000" || fail "payee balance is not 0.1 ERG"

echo "[op] 6. control: a made-up receipt asked through the relay is proven absent"
o="$(in3 wallet receive --state "$W/payee" --node "p2p://$T1ADDR" --ids 00000000000000000000000000000000000000000000000000000000000000ff)"; echo "$o" | head -n 3 | sed 's/^/  /'
echo "$o" | grep -q "ABSENT" || fail "made-up receipt not proven absent"
kill "$SERVE" 2>/dev/null; wait "$SERVE" 2>/dev/null; sed 's/^/  relay: /' "$W/serve.log"
echo "  relaying wallet stopped: $(ip netns exec "${NS[T1]}" ss -ltn 2>/dev/null | grep -c ":9030 ") listener(s) left on its port"

echo "[op] 7. control: a serving wallet without relay on gives the payee nothing"
ip netns exec "${NS[T1]}" "$T" serve --state "$W/relay" --listen "$T1ADDR" --secs 14 --net-file "$NETF" > "$W/serve-norelay.log" 2>&1 &
SERVE=$!; sleep 2
o="$(in3 wallet receive --state "$W/payee" --node "p2p://$T1ADDR" --ids "$receipt" 2>&1)"; echo "$o" | head -n 2 | sed 's/^/  /'
echo "$o" | grep -q "NOT PROVEN" || fail "an answer came from a wallet that does not relay"
wait "$SERVE"; sed 's/^/  no-relay: /' "$W/serve-norelay.log"
echo "[op] verdict: $rig_verdict"
