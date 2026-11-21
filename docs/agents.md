# tachys for programs and agents

An agent that needs a wallet should not need to know how one works. Run `tachys recipes` once: it prints one JSON
object listing, for each thing a program might want, the one command to run and what it gives back (about 4 KB;
no network, no wallet needed). Every command below prints exactly one JSON object and exits 0 (done), 1 (refused,
failed, or not yet so) or 2 (misuse). Nothing here ever prints a key. The default network is mainnet.

```
tachys recipes                                  what can be done, each as one command
tachys setup   --state DIR                      make the wallet if there is none; says where to pay it   (no network)
tachys address --state DIR                      where to be paid                                         (no network)
tachys balance --state DIR                      what this wallet holds, proven; payments still waiting
tachys pay     --state DIR --to ADDRESS --erg 0.02 [--purpose TEXT] [--dry-run]
tachys accept  --state DIR --receipt BOXID      is this receipt real and mine?
tachys wait    --state DIR --receipt BOXID [--confirmations N] [--timeout SECONDS]    until paid
tachys wait    --state DIR --tx TXID [--timeout SECONDS]                              until a payment I made is settled
tachys history --state DIR                      what was sent and what became of it                      (no network)
```

`wait` is one call in place of a loop of them: it returns as soon as the receipt is accepted with the confirmations
asked for, or the payment is in a block with them, and exits 1 otherwise (see below).

If `pay` answers `outcome_unknown: true`, a node was sent the payment and did not answer. Do not pay again: run
`tachys wait --tx` with the `tx` it gave. The wallet keeps the payment as waiting, sends it again if it did not
arrive, and gives its coins back if it is not in a block after ten.

Add `--network testnet` for testnet. With no `--node`, usable nodes are found
automatically (`tachys nodes --state DIR` shows which): a few well-known nodes are tried first and asked which peers
advertise a REST address; a node is used only if it is on this network, at the chain's height, holds the UTXO set and
indexes boxes; nodes advertised by peers are used only when fewer than two well-known ones answer. `--node URL[,URL...]`
names the nodes instead. Either way a node that does not answer is passed over for the next.
Every other command answers the same way with `--json` (`docs/json.md`), and the commands above are built on those answers.

## What each returns
Every answer carries `v` (now 1) and `ok`, which is true exactly when the exit code is 0. A refusal carries
`refused` (why, in words) and `code` (a word to branch on): `no_wallet`, `no_policy`, `policy` (a limit; retrying
will not help), `no_tip`, `no_node`, `not_proven` (the network; try again later), `not_yet` (ask again after a
block), `timeout`, `node_refused`, `outcome_unknown`, `misuse`, `failed`.

- `balance`: `{"ok":true,"proven":true,"complete":true,"address":…,"erg":"0.564200000","nanoerg":564200000,"unconfirmed_nanoerg":0,"tokens":[{"id":…,"amount":…}],"boxes":2,"waiting":[],"settled":[],"tip_height":…,"network":…}`.
  The chain tip came from several peers and was checked for work. The nodes were asked what the wallet's addresses
  hold, and every box counted was proven unspent against that chain. `nanoerg`, `erg`, `boxes` and `tokens` count only
  boxes confirmed under that tip, which is what can be spent; `unconfirmed_nanoerg` is what sits beside them, not
  inside them, in boxes proven only in a block one node has shown above it. `complete` is false if a node did not answer
  for an address. `waiting` lists payments of this wallet not yet in a block (their coins are held back);
  `settled` what became of any that were settled by this call.
- `pay`: `{"ok":true,"tx":…,"receipt":…,"to":…,"nanoerg":…,"fee_nanoerg":1100000,"purpose":…}`. The
  payment is broadcast, not yet in a block. Give `receipt` to the payee.
  A refusal looks like `{"ok":false,"refused":"over the per-payment limit: …"}`.
- `accept`: `{"ok":true,"accepted":true,"reason":…,"confirmations":N,"receipt":{"box":…,"nanoerg":…,"tokens":[…],"first_seen":true,"pending":null}}`
  plus the balance fields. A receipt is accepted only if the box is proven unspent, under a tip the peers agree
  on, and pays this wallet. **That is all `accepted` says.** Whether it is the payment you were owed is for you to
  check: `receipt.nanoerg` and `receipt.tokens` are what that one box holds, and `receipt.first_seen` is false for
  a box this wallet already knew (the same receipt shown twice, or the wallet's own change). Until the payment
  is in a block the answer is `ok:false`, `accepted:false`, exit 1, `code:"not_yet"`; so it is while only the node
  has shown the block it is in. Ask again after a block (about two minutes on mainnet), or use `wait`.
  `confirmations` is a lower bound; for a large payment wait for more. One confirmation can be undone: if a longer
  chain without that block takes over, the answer turns to `not_yet` with a `refused` that says the box was undone
  (its state in `wallet receive` is `undone`). Treat a payment you counted as not paid until it is accepted again;
  when it is, `first_seen` is false, because it is the same payment back. If you held the signed payment yourself
  (`wallet promise`) and its coins were spent elsewhere first, the wallet's sync finds so and the answer becomes
  `code:"failed"`: it will never be paid by that payment. Given only the box id, `accept` asks the nodes whether one
  holds, in its pool, the payment that makes that box. The wallet holds it only if, as read, it makes that box, every
  coin it spends is proven unspent, and its signatures verify against them. `receipt.pending` then says what is
  known: `true`, such a payment is held, its coins proven unspent and its signatures verified, so it is on its way
  (`true` also for a box your own payment makes, still waiting, such as its change: this wallet built and signed it
  from coins it had proven unspent, and each check asks about them again; the reason says it is your own);
  `false`, the nodes that answered hold no such payment (not sent, not yet seen, or dropped); `null`, no node
  answered, or there is nothing to say. A payment you held with `wallet promise` was taken as handed over, with
  nothing about it checked: its receipt answers `pending: null`, not `true`, until it is in a block. Once held, a coin of it
  spent elsewhere makes the answer `failed`, unless the payment is proven to be in the chain; and if what spent the
  coin is itself undone, it is held again. A payment dropped from every pool before it was first asked about cannot
  be told from one never sent. A coin of a held payment that this wallet never saw (one made by a payment not yet
  in a block) is not taken for gone: the answer stays `not_yet`. A reorganisation deeper than the headers the wallet
  keeps (ten to sixteen) reads as `spent`, and `accept` then says `failed` with "has since spent it", though you did
  not.
- `wait`: `--receipt BOXID` answers as `accept` does, once the receipt has `--confirmations` (1 unless said). `--tx
  TXID` answers `{"tx":…,"result":"confirmed","confirmations":N}` once a payment this wallet made has them. It exits
  1 otherwise: with `code:"timeout"` if the time ran out (15 minutes unless `--timeout` says otherwise), at once if
  the receipt pays someone else, its payment can never confirm (`failed`), or the payment ended as `coins spent` or
  `not included`.
- `setup`: `{"created":…,"address":…,"network":…,"sealed":…,"can_pay":…,"note":…}`. `history`:
  `{"address":…,"sent":[{"tx":…,"result":…}],"expecting":[…],"payments_log":[…]}`, as the last sync left them.
- Token payments: `--token ID --token-amount RAW` in place of `--erg` (raw units, no decimals). The
  policy must allow the token and set its limits.
- `pay --dry-run` answers what would leave the wallet (`would_leave_wallet_nanoerg`, fee included)
  or why it would be refused, and signs nothing. On a sealed wallet given no password it still answers: it previews
  the policy's verdict without unlocking the seed phrase (a wrong password, by contrast, is refused).

## The policy file: limits on every signature
With `DIR/policy.json` in place, every command that signs is held to it, not only `pay`: `wallet send`,
`consolidate`, `burn`, `babel-offer` and a signing request (`wallet request --sign`). With no policy
file, `pay` is off. The limits are applied to what the transaction about to be signed takes out of
the wallet, read off the transaction itself: what its boxes put in, less what comes back to it.

```json
{ "max_per_payment_nanoerg": 50000000, "max_per_day_nanoerg": 100000000, "allowed_payees": [],
  "allow_tokens": false, "token_limits": { "TOKEN_ID": { "per_payment": 500, "per_day": 2000 } } }
```

- `max_per_payment_nanoerg`, `max_per_day_nanoerg`: hard caps on the nanoERG that leaves, **the
  network fee included** (a payment of 0.02 ERG counts as 0.0211). A limit left out is zero.
- `allowed_payees`: if not empty, every output that is not the wallet's own or the fee must go to one
  of these addresses.
- `allow_tokens` and `token_limits`: a token leaves the wallet (paid, burned, or paid as a fee) only
  if `allow_tokens` is true and the token has its own `per_payment` and `per_day`, in raw units.
- `DIR/payments.log` must exist: create an empty one beside the policy. A missing log is a refusal,
  not a day with nothing spent, and so is a log with a line that cannot be read.
- `wallet mnemonic` is refused while the policy file is there.

The entry is written to the log before the signature is made, under a lock, so two payments at once
cannot both pass on the same daily total. A payment that then fails still counts against the day.

An agent can be talked into paying. These limits are the defence against that, and they hold only
where the agent cannot go round them:

- **With no policy file, `wallet send` signs without limits.** That is how a person uses the wallet.
  So the limits are only as strong as the agent's inability to remove the file, or to copy the
  wallet somewhere that has none.
- **A program running as the wallet's own user can do both.** It can read the wallet file and the
  password, so it can copy the wallet to a directory without a policy, or open the seed phrase
  without tachys at all. File permissions inside the wallet directory do not change this: the
  directory's owner can delete any file in it, sticky bit or not.
- **So the agent must not be that user.** Keep `DIR`, the password, `policy.json` and `payments.log`
  under a separate account that the agent cannot read, and give the agent one way in: a small
  wrapper, run as that account (a `sudo` rule naming the wrapper and nothing else), that accepts
  only `setup`, `address`, `balance`, `pay`, `accept`, `wait` and `history`, sets `--state DIR` and the password
  itself, and passes the rest on. Then nothing the agent is told can reach the key, the policy or the log.
- If the agent does run as the wallet's user, treat the limits as a guard against mistakes, not
  against a determined caller: keep only a small float in the wallet, and send anything larger to
  a person.

## The log
`DIR/payments.log`, one JSON object per line. Each signature writes a line before it is made: when,
to whom, how much left the wallet (`nanoerg`, fee included), any tokens, the purpose given and the
command. A second line follows with the transaction id and the receipt; it counts for nothing.
`--purpose` is the place to say why.

## Limits to know
- Every node used sees the addresses looked up and the payments sent. A node can hide a box or drop a payment; it
  cannot invent a box (each is proven against the tip the peers agree on).
- The seed phrase is sealed in `DIR/wallet.json` only if a password was given when the wallet was made
  (`TACHYS_PASS=… tachys wallet new --state DIR`, or `wallet encrypt` for an existing one). `pay` then needs
  `TACHYS_PASS` in its environment; `balance` and `accept` do not. Without a password the phrase is in
  plain text: protect the directory.
- An offline or unconfirmed receipt is a promise, not a payment (`docs/design.md`).
- A payment "not included" after ten blocks is given up, and its coins count as the wallet's again; but its signed
  transaction is still valid and could yet be included. So the wallet's next payment always spends one of those
  coins: of the two, only one can ever confirm. Paying again is therefore safe, and may make the first one void.
- If a node was sent a payment and did not answer, or answered with a fault of its own, the payment is kept as
  waiting even if another node then refuses it (the first may hold it). Its coins are held back for up to ten
  blocks. That is the cautious reading, and it costs a wait.
- Nothing is read against a chain more than three hours old: if the chain itself stalls that long, every command
  that needs a proof refuses until blocks come again.
- A wallet restored from a phrase is looked over at twenty addresses, once. If none holds a coin then, only the
  first is kept: a payment made later to another of them is not seen until the wallet is restored again.
