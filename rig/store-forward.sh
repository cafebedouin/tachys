# tachys store-and-forward recipe, a hook for the peeryard rig (sourced by rig.sh inside its namespace).
# A wallet that has lost its connection signs a payment, hands it to another wallet, and that wallet
# submits it when it can. No REST call is made by any wallet in this recipe.
#   A  = v6.0.8 + node-patches/0001, mining          T1 = carrier wallet (linked to A and T2)
#   T2 = payer wallet (linked to A and T1; its link to A is cut for the offline phase)
# Run:  SCRATCH=<dir> TACHYS_ROOT=<tachys checkout> TACHYS_JAR_PATCHED=<jar> \
#         bash <peeryard>/rig/rig.sh $TACHYS_ROOT/rig/store-forward.json $TACHYS_ROOT/rig/store-forward.sh
T="$TACHYS_ROOT/target/release/tachys"; W="$SCRATCH/tachys"; mkdir -p "$W"; NETF="$W/net.json"
in1() { ip netns exec "${NS[T1]}" "$T" "$@" --net-file "$NETF"; }
in2() { ip netns exec "${NS[T2]}" "$T" "$@" --net-file "$NETF"; }
fail() { echo "[sf] FAIL: $*"; rig_verdict=FAIL; }
rig_verdict=PASS
A1="${LIP["A,${LINK_OF["A,T1"]}"]}:${P2P[A]}"; A2="${LIP["A,${LINK_OF["A,T2"]}"]}:${P2P[A]}"; T1ADDR="${LIP["T1,${LINK_OF["T1,T2"]}"]}:9030"

end=$((SECONDS+240)); while [[ $SECONDS -lt $end ]]; do [[ "$(full_height A)" -ge 20 ]] && break; sleep 4; done
gen="$(header_at A 1)"; cpid="$(header_at A $(( $(full_height A) - 12 )))"; rest A "/blocks/$cpid/header" > "$W/cp.json"
jq -n --arg g "$gen" --argjson magic "$MAGIC" --slurpfile cp "$W/cp.json" \
  '{name:"devnet", magic:$magic, genesis:$g, checkpoint:$cp[0], floor_divisor:1048576, v2_activation_height:0, block_interval_ms:3000, seeds:""}' > "$NETF"
echo "$A1" > "$W/seeds-a-for-t1.txt"; echo "$A2" > "$W/seeds-a-for-t2.txt"; echo "$T1ADDR" > "$W/seeds-t1.txt"

echo "[sf] 1. online: the payer wallet is created, paid by the node's wallet, and proves its box"
in2 tips --seeds "$W/seeds-a-for-t2.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payer" > /dev/null
addr="$(in2 wallet new --state "$W/payer" | sed -n 's/^new wallet: address //p')"
wait_balance A 2000000000 240 >/dev/null
txid="$(wallet A /wallet/payment/send "[{\"address\":\"$addr\",\"value\":500000000}]" | tr -d '"')"
box=""; end=$((SECONDS+120)); while [[ $SECONDS -lt $end && -z "$box" ]]; do sleep 6
  box="$(wallet A "/wallet/transactionById?id=$txid" | jq -r --arg a "$addr" 'select(.numConfirmations != null and .numConfirmations >= 1) | .outputs[] | select(.address==$a) | .boxId' 2>/dev/null | head -1)"; done
feetree="$(wallet A "/wallet/transactionById?id=$txid" | jq -r '.outputs[] | select(.value==1000000) | .ergoTree' | head -1)"
jq --arg f "$feetree" '. + {fee_tree:$f}' "$NETF" > "$NETF.tmp" && mv "$NETF.tmp" "$NETF"
in2 tips --seeds "$W/seeds-a-for-t2.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payer" > /dev/null
o="$(in2 wallet receive --state "$W/payer" --node "p2p://$A2" --ids "$box")"; echo "$o" | tail -n 2 | sed 's/^/  /'
echo "$o" | grep -q "(added)" || fail "payer could not prove its funding box"

echo "[sf] 2. the payer loses its connection to the node"
partition A T2
c="$(in2 tips --seeds "$W/seeds-a-for-t2.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payer-probe" 2>&1 | tail -n 1)"; echo "  payer -> node: $c"
echo "$c" | grep -q "^verdict: Agreed\|^state root" && fail "the payer still reaches the node"

echo "[sf] 3. offline, the payer signs 0.1 ERG to the node's address and writes a parcel"
dest="$(address A)"
o="$(in2 wallet send --state "$W/payer" --to "$dest" --nanoerg 100000000 --parcel-out "$W/parcel.json")"; echo "$o" | sed 's/^/  /'
[[ -s "$W/parcel.json" ]] || fail "no parcel written"
ptx="$(jq -r .tx_id "$W/parcel.json")"

echo "[sf] 4. the carrier wallet syncs, serves, and takes the parcel from the payer"
in1 tips --seeds "$W/seeds-a-for-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/carrier" > /dev/null
in1 serve --state "$W/carrier" --listen "$T1ADDR" --secs 10 --accept-parcels > "$W/serve.log" 2>&1 &
SERVE=$!; sleep 2
o="$(in2 handoff --peer "$T1ADDR" --parcel "$W/parcel.json")"; echo "  $o"
echo "$o" | grep -q "acknowledged tx $ptx" || fail "handoff not acknowledged"
wait "$SERVE"; sed 's/^/  carrier: /' "$W/serve.log"

echo "[sf] 4b. controls: a carrier must refuse bad parcels before anything reaches the node"
# (a) the same payment with one byte of its spending proof flipped (the id does not cover proofs, so it is taken)
# (b) the same payment with one byte of its input box id flipped (a box that does not exist)
python3 - "$W/parcel.json" "$W/parcel-badproof.json" "$W/parcel-badinput.json" <<'PY'
import json, sys
p = json.load(open(sys.argv[1])); h = p["tx_bytes"]
def flip(hexs, byte):
    i = 2 * byte; return hexs[:i] + ("%02x" % (int(hexs[i:i+2], 16) ^ 1)) + hexs[i+2:]
json.dump({"tx_id": p["tx_id"], "tx_bytes": flip(h, 1 + 32 + 1 + 5)}, open(sys.argv[2], "w"))   # inside the first input's proof
json.dump({"tx_id": p["tx_id"], "tx_bytes": flip(h, 1 + 5)}, open(sys.argv[3], "w"))            # inside the first input's box id
PY
for bad in badproof badinput; do
  cp -r "$W/carrier" "$W/carrier-$bad"; rm -rf "$W/carrier-$bad/outbox"
  in1 serve --state "$W/carrier-$bad" --listen "$T1ADDR" --secs 8 --accept-parcels > "$W/serve-$bad.log" 2>&1 &
  SERVE=$!; sleep 2
  in2 handoff --peer "$T1ADDR" --parcel "$W/parcel-$bad.json" | sed 's/^/  /'
  wait "$SERVE"
  in1 tips --seeds "$W/seeds-a-for-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/carrier-$bad" > /dev/null
  o="$(in1 outbox --state "$W/carrier-$bad" --peer "$A1")"; echo "$o" | sed "s/^/  [$bad] /"
  echo "$o" | grep -q "REFUSED before submission" || fail "carrier did not refuse the $bad parcel"
  echo "$o" | grep -q "outbox done: 0 submitted, 1 refused" || fail "carrier submitted the $bad parcel"
done
echo "  node mempool holds the payment: $(rest A "/transactions/unconfirmed/byTransactionId/$ptx" | jq -r 'if .id then "YES" else "no" end' 2>/dev/null)"

echo "[sf] 5. the carrier submits what it carries to the node over the peer protocol"
in1 tips --seeds "$W/seeds-a-for-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/carrier" > /dev/null
o="$(in1 outbox --state "$W/carrier" --peer "$A1")"; echo "$o" | sed 's/^/  /'
echo "$o" | grep -q "delivered on request" || fail "the node did not take the payment"

echo "[sf] 6. the payment confirms"
conf=""; end=$((SECONDS+90)); while [[ $SECONDS -lt $end && -z "$conf" ]]; do sleep 5
  conf="$(wallet A "/wallet/transactionById?id=$ptx" | jq -r 'select(.numConfirmations != null and .numConfirmations >= 1) | .numConfirmations' 2>/dev/null)"; done
[[ -n "$conf" ]] && echo "  node's wallet sees tx $ptx with $conf confirmation(s)" || fail "payment never confirmed"

echo "[sf] 6b. control: the same parcel handed over again after it confirmed is refused as already on chain"
cp -r "$W/carrier" "$W/carrier-replay"; rm -rf "$W/carrier-replay/outbox"
in1 serve --state "$W/carrier-replay" --listen "$T1ADDR" --secs 8 --accept-parcels > "$W/serve-replay.log" 2>&1 &
SERVE=$!; sleep 2
in2 handoff --peer "$T1ADDR" --parcel "$W/parcel.json" | sed 's/^/  /'
wait "$SERVE"
in1 tips --seeds "$W/seeds-a-for-t1.txt" --peers 1 --tries 1 --quorum 1 --state "$W/carrier-replay" > /dev/null
o="$(in1 outbox --state "$W/carrier-replay" --peer "$A1")"; echo "$o" | sed 's/^/  [replay] /'
echo "$o" | grep -q "already on chain" || fail "carrier did not recognise a payment that is already on chain"

echo "[sf] 7. the payer reconnects and finds its change without asking for it by address"
heal A T2; sleep 3
in2 tips --seeds "$W/seeds-a-for-t2.txt" --peers 1 --tries 1 --quorum 1 --state "$W/payer" > /dev/null
o="$(in2 wallet status --state "$W/payer" --node "p2p://$A2")"; echo "$o" | sed 's/^/  /'
echo "$o" | grep -q "balance 0.3989" || fail "payer balance is not 0.3989 ERG"
echo "[sf] verdict: $rig_verdict"
