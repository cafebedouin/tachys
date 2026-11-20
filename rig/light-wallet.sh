# tachys light-wallet recipe, a hook for the peeryard rig (sourced by rig.sh inside its namespace).
#   A = v6.0.8 + node-patches/0001 (box proofs over the peer protocol), mining
#   B = the v6.0.8 release, following A
# Run:  SCRATCH=<dir> TACHYS_ROOT=<tachys checkout> TACHYS_JAR_PATCHED=<jar> TACHYS_JAR_RELEASE=<jar> \
#         bash <peeryard>/rig/rig.sh $TACHYS_ROOT/rig/light-wallet.json $TACHYS_ROOT/rig/light-wallet.sh
# Checks, in order:
#   1 tip sync from both nodes            2 a payment from A's wallet accepted by receipt over the peer protocol
#   3 the same request to unpatched B     4 what B does to a peer that sends it the unknown message
#   5 tachys pays back and finds its change
T="$TACHYS_ROOT/target/release/tachys"; W="$SCRATCH/tachys"; mkdir -p "$W"; NETF="$W/net.json"
t() { ip netns exec "${NS[A]}" "$T" "$@" --net-file "$NETF"; }
fail() { echo "[tachys] FAIL: $*"; rig_verdict=FAIL; }
rig_verdict=PASS

echo "[tachys] waiting for A to reach height 20 and B to follow"
end=$((SECONDS+300)); while [[ $SECONDS -lt $end ]]; do
  hA="$(full_height A)"; sc="$(same_chain A B)"
  [[ "${hA:-0}" -ge 20 && "$sc" == SAME@* ]] && break; sleep 5
done
echo "[tachys] A.h=$(full_height A) B.h=$(full_height B) ${sc%%:*}"

# the private network's constants, as a wallet release would carry them
gen="$(header_at A 1)"; cph=$(( $(full_height A) - 12 )); cpid="$(header_at A "$cph")"
rest A "/blocks/$cpid/header" > "$W/cp.json"
printf '%s:%s\n%s:%s\n' "${IP[A]}" "${P2P[A]}" "${IP[B]}" "${P2P[B]}" > "$W/seeds.txt"
jq -n --arg g "$gen" --arg seeds "$W/seeds.txt" --argjson magic "$MAGIC" --slurpfile cp "$W/cp.json" \
  '{name:"devnet", magic:$magic, genesis:$g, checkpoint:$cp[0], floor_divisor:1048576, v2_activation_height:0, block_interval_ms:3000, epoch_length:16, seeds:$seeds}' > "$NETF"
echo "[tachys] checkpoint height $cph, header version $(jq -r .version "$W/cp.json"), peers $(tr '\n' ' ' < "$W/seeds.txt")"

echo "[tachys] 1. tip sync"
t tips --peers 2 --tries 2 --quorum 1 --state "$W/wallet" | tail -n 6
t tips --peers 2 --tries 2 --quorum 1 --state "$W/wallet" | grep -q "^verdict: Agreed" || fail "tip sync"

echo "[tachys] 2. payment from A's wallet, accepted by receipt over the peer protocol"
addr="$(t wallet new --state "$W/wallet" | sed -n 's/^new wallet: address //p')"
echo "  tachys address: $addr"
wait_balance A 2000000000 240 >/dev/null
txid="$(wallet A /wallet/payment/send "[{\"address\":\"$addr\",\"value\":500000000}]" | tr -d '"')"
echo "  A paid 0.5 ERG, tx $txid"
box=""; end=$((SECONDS+120)); while [[ $SECONDS -lt $end && -z "$box" ]]; do
  sleep 6
  box="$(wallet A "/wallet/transactionById?id=$txid" | jq -r --arg a "$addr" 'select(.numConfirmations != null and .numConfirmations >= 1) | .outputs[] | select(.address==$a) | .boxId' 2>/dev/null | head -1)"
done
[[ -n "$box" ]] || fail "payment never confirmed"
# this network's fee script (its reward delay is not mainnet's), read from the node's own payment
feetree="$(wallet A "/wallet/transactionById?id=$txid" | jq -r '.outputs[] | select(.value==1000000) | .ergoTree' | head -1)"
jq --arg f "$feetree" '. + {fee_tree:$f}' "$NETF" > "$NETF.tmp" && mv "$NETF.tmp" "$NETF"
echo "  fee script taken from the node's payment: ${feetree:0:24}... (${#feetree} hex chars)"
echo "  receipt (box id): $box"
t tips --peers 2 --tries 2 --quorum 1 --state "$W/wallet" > /dev/null
out="$(t wallet receive --state "$W/wallet" --node "p2p://${IP[A]}:${P2P[A]}" --ids "$box")"; echo "$out" | sed 's/^/  /'
echo "$out" | grep -q "(added)" || fail "receipt not accepted from patched A"

echo "[tachys] 3. the same request to the unpatched release node B"
outB="$(t wallet receive --state "$W/wallet" --node "p2p://${IP[B]}:${P2P[B]}" --ids "$box" 2>&1)"; echo "$outB" | sed 's/^/  /'
echo "$outB" | grep -q "NOT PROVEN" || fail "unpatched B unexpectedly answered"

echo "[tachys] 4. how B treated the peer that sent the unknown message"
sleep 3
echo "  B blacklisted peers: $(rest B /peers/blacklisted | jq -c . 2>/dev/null)"
echo "  B log lines mentioning the message, a penalty or a ban:"
grep -iE "code 92|penal|blacklist|ban|unknown message|No handler" "$RIG_LOG_DIR/node_B.log" | tail -n 8 | cut -c1-220 | sed 's/^/    /'
t tips --peers 2 --tries 2 --quorum 1 --state "$W/wallet" | grep -E "^  ${IP[B]}" | cut -c1-150 | sed 's/^/  after: /'

echo "[tachys] 5. tachys pays 0.1 ERG back to A and finds its change"
t tips --peers 2 --tries 2 --quorum 1 --state "$W/wallet" > /dev/null
t wallet status --state "$W/wallet" --node "p2p://${IP[A]}:${P2P[A]}" | tail -n 3 | sed 's/^/  /'
sendout="$(t wallet send --state "$W/wallet" --node "http://127.0.0.1:${REST[A]}" --to "$(address A)" --nanoerg 100000000 --broadcast 2>&1)"; echo "$sendout" | sed 's/^/  /'
echo "$sendout" | grep -q "accepted" || fail "payment from tachys not accepted"
sleep 20
t tips --peers 2 --tries 2 --quorum 1 --state "$W/wallet" > /dev/null
st="$(t wallet status --state "$W/wallet" --node "p2p://${IP[A]}:${P2P[A]}")"; echo "$st" | sed 's/^/  /'
echo "$st" | grep -q "balance 0.3989" || fail "balance after paying back is not 0.3989 ERG"
echo "[tachys] verdict: $rig_verdict"
