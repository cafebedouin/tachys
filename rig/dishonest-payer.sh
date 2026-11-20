# tachys dishonest-payer recipe, a hook for the peeryard rig (sourced by rig.sh inside its namespace).
# A payee is handed a receipt by a payer who then spends the same coins elsewhere, twice over:
#   part 1, before the payment reaches any block (a double-spent receipt);
#   part 2, after it has a confirmation, by mining the other spend on a longer chain cut off from the payee's
#           node and then rejoining it (a reorganisation that undoes a confirmed payment).
# The cheat is a second copy of the payer's wallet (the same phrase in another state directory), which knows
# nothing of the first copy's payment. Wallets ask for proofs over the peer protocol; payments reach a node's
# mempool by REST, so that how a payment travels is not what is tested.
#   A  = v6.0.8 + node-patches/0001, mining every 2 s   B  = the same, mining only once A has stopped (part 2), so that
#        difficulty stays steady: the devnet's retargeting is not mainnet's, and the wallet holds headers to mainnet's
#   T1 = the payer and the payee (reach A only)          T2 = the payer's second copy (reaches B only)
# Run:  SCRATCH=<dir> TACHYS_ROOT=<tachys checkout> TACHYS_JAR_PATCHED=<jar> \
#         bash <peeryard>/rig/rig.sh $TACHYS_ROOT/rig/dishonest-payer.json $TACHYS_ROOT/rig/dishonest-payer.sh
T="${TACHYS_BIN:-$TACHYS_ROOT/target/release/tachys}"; W="$SCRATCH/tachys"; mkdir -p "$W"; NETF="$W/net.json"
in1() { ip netns exec "${NS[T1]}" "$T" "$@" --net-file "$NETF"; }
in2() { ip netns exec "${NS[T2]}" "$T" "$@" --net-file "$NETF"; }
fail() { echo "[dp] FAIL: $*"; rig_verdict=FAIL; }
rig_verdict=PASS
A1="${LIP["A,${LINK_OF["A,T1"]}"]}:${P2P[A]}"; B2="${LIP["B,${LINK_OF["B,T2"]}"]}:${P2P[B]}"
tip1() { in1 tips --seeds "$W/seeds-a.txt" --peers 1 --tries 1 --quorum 1 --state "$1" >> "$W/tips.log" 2>&1 || echo "  (tips refused for $1: $(tail -n 1 "$W/tips.log"))" >&2; }
tip2() { in2 tips --seeds "$W/seeds-b.txt" --peers 1 --tries 1 --quorum 1 --state "$1" >> "$W/tips.log" 2>&1 || echo "  (tips refused for $1: $(tail -n 1 "$W/tips.log"))" >&2; }
# prove NS STATE BOX: the wallet asks its node (A from T1, B from T2) about one box, again a block later while the
# node is ahead of the agreed tip; prints the box's line of `checked` once it is proven, or the last answer.
# With PROVE_ONCE set it asks once.
prove() { local i o node; [[ "$1" == 1 ]] && node="$A1" || node="$B2"
  for i in 1 2 3 4 5 6 7 8; do "tip$1" "$2"
    o="$("in$1" wallet receive --state "$2" --node "p2p://$node" --ids "$3" --json 2>/dev/null | jq -c --arg b "$3" '{checked: [.checked[]? | select(.id==$b) | {state, confirmations, height}]}')"
    [[ "$(jq -r '.checked[0].state // ""' <<<"$o")" == proven || -n "${PROVE_ONCE:-}" ]] && break; sleep 6; done
  echo "$o"; }
st() { jq -r '.checked[0].state // "none"' <<<"$1"; }
utxo() { ip netns exec "${NS[$1]}" curl -s -o /dev/null -w '%{http_code}' --max-time 4 "http://127.0.0.1:${REST[$1]}/utxo/byId/$2"; }
on_chain() { [[ "$(utxo "$1" "$2")" == 200 ]]; }
off_chain() { [[ "$(utxo "$1" "$2")" == 404 ]]; }
until_ok() { local end=$((SECONDS+$1)); shift; while [[ $SECONDS -lt $end ]]; do "$@" && return 0; sleep 3; done; return 1; }
fund() { # fund ADDRESS: the node's wallet on A pays it 0.5 ERG; prints the new box's id once it has a confirmation
  local txid box="" end; txid="$(wallet A /wallet/payment/send "[{\"address\":\"$1\",\"value\":500000000}]" | tr -d '"')"; echo "$txid" > "$W/last-fund-tx"
  end=$((SECONDS+120)); while [[ $SECONDS -lt $end && -z "$box" ]]; do sleep 4
    box="$(wallet A "/wallet/transactionById?id=$txid" | jq -r --arg a "$1" 'select(.numConfirmations != null and .numConfirmations >= 1) | .outputs[] | select(.address==$a) | .boxId' 2>/dev/null | head -1)"; done
  echo "$box"; }
# tpay NS STATE TO NANOERG NODE: the wallet signs a payment as a parcel (it sends nothing itself), and the parcel's
# transaction goes to NODE's mempool by REST; prints the receipt, and keeps what the node said in STATE/node-said.txt
tpay() { local o; rm -f "$2/sent.json"; "tip$1" "$2"
  o="$("in$1" wallet send --state "$2" --to "$3" --nanoerg "$4" --parcel-out "$2/sent.json" --json 2>/dev/null)"
  if [[ -s "$2/sent.json" ]]; then wallet "$5" /transactions "$(jq -r .tx_json "$2/sent.json")" > "$2/node-said.txt"; else jq -c '{ok, error}' <<<"$o" > "$2/node-said.txt"; fi
  jq -r '.receipt[0] // empty' <<<"$o"; }

end=$((SECONDS+240)); while [[ $SECONDS -lt $end ]]; do [[ "$(full_height A)" -ge 20 ]] && break; sleep 4; done
gen="$(header_at A 1)"; cpid="$(header_at A $(( $(full_height A) - 12 )))"; rest A "/blocks/$cpid/header" > "$W/cp.json"
jq -n --arg g "$gen" --argjson magic "$MAGIC" --slurpfile cp "$W/cp.json" \
  '{name:"devnet", magic:$magic, genesis:$g, checkpoint:$cp[0], floor_divisor:1048576, v2_activation_height:0, block_interval_ms:3000, epoch_length:16, seeds:""}' > "$NETF"
echo "$A1" > "$W/seeds-a.txt"; echo "$B2" > "$W/seeds-b.txt"
wait_balance A 3000000000 240 >/dev/null

echo "[dp] 0. setup: a payee and a payer; the fee output's script from the payment that funds the payer"
tip1 "$W/payee"; qaddr="$(in1 wallet new --state "$W/payee" --json 2>/dev/null | jq -r .address)"; echo "  payee $qaddr"
tip1 "$W/payer1"; p1addr="$(in1 wallet new --state "$W/payer1" --json 2>/dev/null | jq -r .address)"
box1="$(fund "$p1addr")"; [[ -n "$box1" ]] || fail "the first payer was never funded"
feetree="$(wallet A "/wallet/transactionById?id=$(cat "$W/last-fund-tx")" | jq -r '.outputs[] | select(.value==1000000) | .ergoTree' | head -1)"
jq --arg f "$feetree" '. + {fee_tree:$f}' "$NETF" > "$NETF.tmp" && mv "$NETF.tmp" "$NETF"

echo "[dp] 1. part 1: the payer proves its box; a second copy of its wallet does too, through B"
o="$(prove 1 "$W/payer1" "$box1")"; echo "  payer: $o"; [[ "$(st "$o")" == proven ]] || fail "the payer could not prove its box"
cp -r "$W/payer1" "$W/payer2"
o="$(prove 2 "$W/payer2" "$box1")"; echo "  copy:  $o"; [[ "$(st "$o")" == proven ]] || fail "the copy could not prove the box"

echo "[dp] 2. the payer signs 0.1 ERG to the payee offline and hands over the signed payment; the payee keeps it"
tip1 "$W/payer1"; o="$(in1 wallet send --state "$W/payer1" --to "$qaddr" --nanoerg 100000000 --parcel-out "$W/promise1.json" --json 2>/dev/null)"
r1="$(jq -r '.receipt[0] // empty' <<<"$o")"; echo "  receipt $r1"; [[ -n "$r1" ]] || fail "the payer could not sign: $(jq -c . <<<"$o")"
o="$(in1 wallet promise --state "$W/payee" --tx "$(jq -r .tx_bytes "$W/promise1.json")" --json 2>/dev/null)"; echo "  payee: $(jq -c '{ok, promised, nanoerg}' <<<"$o")"

echo "[dp] 3. the copy spends the same box first: 0.2 ERG to A's address, through B"
# B follows A; a payment signed at A's height is refused by a B a block behind (creation height above its own)
until_ok 60 test "$(full_height B)" -ge "$(full_height A)"
x="$(tpay 2 "$W/payer2" "$(address A)" 200000000 B)"; echo "  B said $(cat "$W/payer2/node-said.txt")"
until_ok 120 off_chain A "$box1" && echo "  A: the payer's box is spent" || fail "the copy's payment never spent the box"
sleep 8

echo "[dp] 4. the payee asks about its receipt (what accept and wait --receipt read)"
o="$(PROVE_ONCE=1 prove 1 "$W/payee" "$r1")"; echo "  payee: $o"
s="$(st "$o")"; [[ "$s" == proven || "$s" == seen ]] && fail "a double-spent receipt was proven"
echo "  [observe] receipt state: $s"

echo "[dp] 5. the payee tries to submit the payment it holds, and syncs"
tip1 "$W/payee"; in1 outbox --state "$W/payee" --peer "$A1" 2>&1 | sed 's/^/  outbox: /'
ls "$W/payee/outbox" "$W/payee/given-up" 2>/dev/null | sed 's/^/  payee dir: /'
tip1 "$W/payee"; in1 wallet status --state "$W/payee" --node "p2p://$A1" 2>&1 | sed 's/^/  status: /'
o="$(PROVE_ONCE=1 prove 1 "$W/payee" "$r1")"; echo "  [observe] receipt state after the payee's own attempt: $(st "$o")"

echo "[dp] 6. part 2: a fresh payer and a copy of it, funded"
tip1 "$W/payer3"; p3addr="$(in1 wallet new --state "$W/payer3" --json 2>/dev/null | jq -r .address)"
box2="$(fund "$p3addr")"; [[ -n "$box2" ]] || fail "the second payer was never funded"
o="$(prove 1 "$W/payer3" "$box2")"; echo "  payer: $o"; [[ "$(st "$o")" == proven ]] || fail "the second payer could not prove its box"
cp -r "$W/payer3" "$W/payer4"
o="$(prove 2 "$W/payer4" "$box2")"; echo "  copy:  $o"; [[ "$(st "$o")" == proven ]] || fail "the second copy could not prove the box"

echo "[dp] 7. A and B are cut apart; the payer pays the payee 0.1 ERG through A"
# B must be on A's chain when they part, or the branch B mines forks further back than the split (a deep reorganisation)
until_ok 120 test "$(same_chain A B | cut -c1-5)" == "SAME@" -a "$(full_height B)" -ge "$(full_height A)" || { relink 60 A B; until_ok 120 test "$(full_height B)" -ge "$(full_height A)"; }
echo "  before the split: A $(full_height A), B $(full_height B), $(same_chain A B | cut -c1-20)"
EVENT_DETAIL="split for the reorganisation" partition A B
r2="$(tpay 1 "$W/payer3" "$qaddr" 100000000 A)"; echo "  receipt $r2; A said $(cat "$W/payer3/node-said.txt")"
until_ok 120 on_chain A "$r2" || fail "the payment never reached a block on A"
h_in="$(full_height A)"; echo "  A: the receipt box is on chain by height $h_in"
until_ok 90 test "$(full_height A)" -ge $((h_in + 2))
o="$(prove 1 "$W/payee" "$r2")"; echo "  payee: $o"
s="$(st "$o")"; c="$(jq -r '.checked[0].confirmations // 0' <<<"$o")"
[[ "$s" == proven && "$c" -ge 1 ]] && echo "  [observe] accept would answer accepted: proven with $c confirmation(s)" || fail "the payment was not proven before the split was undone (state $s)"

echo "[dp] 8. A stops mining; the copy spends the same box through B, which mines a longer branch"
stop_mining A; start_mining B 2s
until_ok 60 test "$(full_height B)" -ge "$(rest A /info | jq -r '.fullHeight // 0')"
x="$(tpay 2 "$W/payer4" "$(address B)" 200000000 B)"; echo "  B said $(cat "$W/payer4/node-said.txt")"
until_ok 120 off_chain B "$box2" || fail "the copy's payment never spent the box on B"
# a shallow reorganisation, as a payee at a till would meet: B ends two blocks past A
until_ok 180 test "$(full_height B)" -gt $(( $(full_height A) + 2 ))
echo "  heights: A $(full_height A), B $(full_height B)"

echo "[dp] 9. A and B are joined again; A takes B's longer chain"
EVENT_DETAIL="rejoin" heal A B
# both were relaunched to swap their mining: their link must be dialled again
relink 90 A B
until_ok 180 off_chain A "$r2"; sleep 6
echo "  $(same_chain A B); A: the receipt box is $(on_chain A "$r2" && echo "still unspent" || echo "gone"); the payer's box is $(on_chain A "$box2" && echo "unspent" || echo "spent")"
# no reorganisation, nothing to judge: that is the rig's doing (the nodes did not reconnect), not the wallet's
noreorg=""; on_chain A "$r2" && { harness_fail "the reorganisation did not happen: A kept its branch"; noreorg=1; [[ "$rig_verdict" == PASS ]] && rig_verdict=INCONCLUSIVE; }

echo "[dp] 10. the payee asks about the receipt it had accepted"
o="$(PROVE_ONCE=1 prove 1 "$W/payee" "$r2")"; echo "  payee: $o"
s="$(st "$o")"; [[ -z "$noreorg" && ( "$s" == proven || "$s" == seen ) ]] && fail "a payment undone by a reorganisation is still proven"
echo "  [observe] receipt state after the reorganisation: $s (undone if the block it was confirmed under is within the payee's stored headers, spent if deeper)"
tip1 "$W/payee"; in1 wallet status --state "$W/payee" --node "p2p://$A1" 2>&1 | sed 's/^/  status: /'
echo "[dp] verdict: $rig_verdict"
