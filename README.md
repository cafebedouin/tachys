# tachys

An experimental light wallet for Ergo, for two users we do not think the other wallets were built around:

1. **A program or an agent that needs a wallet.** It should be able to be paid and to pay without running a node,
   without taking a server's word for its balance, and without spending its context learning how a UTXO chain
   works. Here each thing it wants is one command, each answer is one JSON object, and what it may spend is set
   by its owner in a policy file, which binds an agent that runs as a different user from the wallet's.
2. **A person with a cheap phone and a connection that comes and goes**, who still needs to pay someone standing
   in front of them. Here a payment can be signed with no network and handed over like a cheque; it settles when
   either of them, or a third wallet carrying it, next reaches the network.

Both rest on the same thing: a wallet that asks the network for little and believes only what it can check. It
learns the chain tip from what peers push, accepts it on work and on agreement among network groups, and counts
a coin as its own only with a proof that the box is unspent under that tip.

**Experimental, and not yet a wallet for funds that matter.** "It works" here means it has worked in the runs the
findings log records, not that it is safe to hold money. No person outside the project has reviewed it (the
reviews in the findings log were by other AI models given the code cold), no forged chain has been run against
it, it has no warranty, and it will change. The default network is mainnet, which is real money: use
`--network testnet`, or amounts you can afford to lose. What it trusts is said plainly [below](#what-it-trusts);
what stands between it and real funds is listed in `docs/roadmap.md`. Review is what it most needs.

## For agents

```
cargo build --release -p tachys          # Rust 1.89 or newer; the program is target/release/tachys
# or take the built program for Linux x86_64 from the repository's releases, and check it against SHA256SUMS

tachys recipes                           # everything below as one JSON object (about 4 KB): read it once
tachys setup   --state DIR               # make a wallet, or find the one there; gives the address to be paid at
tachys balance --state DIR               # what it holds, each box proven unspent
tachys pay     --state DIR --to ADDRESS --erg 0.02 --purpose "why"      # gives a tx and a receipt for the payee
tachys wait    --state DIR --tx TX       # one call, until that payment is in a block (or 15 minutes pass)
tachys accept  --state DIR --receipt BOXID                              # was I paid? (a receipt is a box id)
tachys wait    --state DIR --receipt BOXID --confirmations 3            # one call, until I have been
tachys history --state DIR
```

- Each of these commands prints exactly one JSON object and exits 0 (done), 1 (refused, failed, or not yet so) or
  2 (misuse). `ok` is true exactly when the exit code is 0; otherwise `refused` says why in words and `code` is a
  word to branch on (`policy`, `not_yet`, `no_tip`, `timeout`, ...). Add `--network testnet` for testnet.
- **`pay` is off until the wallet's owner allows it**, by writing `DIR/policy.json` and an empty
  `DIR/payments.log`:
  `{"max_per_payment_nanoerg": 50000000, "max_per_day_nanoerg": 100000000, "allowed_payees": [], "allow_tokens": false}`.
  With the file there, every signature is held to it and counted before it is made, and a refusal names the
  limit. The limits bind an agent that runs under another account than the wallet's; one that runs as the
  wallet's own user can go round them (`docs/agents.md`), and the owner's own command, `wallet send`, signs with
  no limit when there is no policy file.
- **`accept` says a box is real, unspent and yours; what it is worth is for you to check.** The answer carries the
  receipt's own `nanoerg` and `tokens`, and `first_seen`: a box you were shown before is not a new payment.
- If `pay` answers `outcome_unknown: true`, a node was sent the payment and did not answer. Do not pay again: run
  `tachys wait --tx` with the `tx` it gave.
- No node to run and none to name: nodes are found, checked and fallen back from automatically.
- Nothing an agent command prints is ever a key. If `TACHYS_PASS` is set when the wallet is made, the seed phrase
  is sealed on disk; otherwise it is there in plain text, and `setup` says so (`sealed: false`).

`docs/agents.md` has the rest; `docs/json.md` has the other commands' fields (all of them take `--json`); a Rust
program can call the wallet in its own process with `tachys::call(args, input)` (in a build that aborts on a
panic, as the phone's does, a malformed proof can still take the caller down with it: `docs/roadmap.md`).

## For a phone

`android/` is the app: several wallets, send and receive with ERG and tokens, receipts as QR codes, payments
signed offline and sent later, signing requests from other sites, all on the same core. It has run on a Galaxy
Note 2 from 2012; the core is 2.7 MB and starts in 16 ms there. `docs/android.md` says what has been tried
on a phone and what has not: the released build was tried by hand on the Note 2 by the maintainer, and on no
other phone. A release carries an APK for each kind of phone, signed with the project's key (`SECURITY.md` gives
its fingerprint), and the Linux program; the checksums are beside them. They are the maintainer's builds, not
reproducible ones: building your own is as good, and for money, better.

## The payments it was designed around

- **Online:** the payer sends; the payee is handed a receipt (a box id) and checks it against the chain itself.
  That check needs no address lookup and no index, and tells a node nothing beyond the one box asked about. (A
  sync that asks what the wallet's addresses hold, as the app's does, does tell the nodes those addresses.)
- **Payer offline:** the payer signs and hands over the signed payment (a file, or a QR code if it fits). The
  payee keeps it and submits it when it can. Until it is in a block it is a promise, like a cheque, and both
  wallets say so.
- **Neither can reach a node:** a third wallet carries the signed payment and submits it, or passes a proof
  request on to its node and the answer back. It can do neither any harm: the payment cannot be altered and the
  proof is checked by the one who asked. (Relaying a proof needs a node carrying the patch in `node-patches/`;
  this has run on the rig only.)

## What it trusts

- **A checkpoint shipped with each release.** A tip is accepted if it links back ten headers of real work above
  a difficulty floor that decays from that checkpoint, and peers in several network groups have reached it. This
  is checkpointed SPV, not full NiPoPoW security (see below).
- **Work, as far as ten headers go.** Tips are ranked by the difficulty they claim; a lighter chain is refused
  while a heavier tip is heard; the floor remembers the last tip this wallet agreed on. A header a single node adds
  above the agreed tip must follow it at the chain's own difficulty (at an epoch's first block, at least half of it,
  less the compact form's rounding of one part in 2^15). Next to the agreed tip that is the remembered floor's own
  bound, give or take that rounding; an honest fall of the largest size can land in that window, so it is allowed. With every connection
  in an attacker's hands a false tip still costs about ten headers at that floor, so a large incoming payment
  should wait for more confirmations.
- **One block's work, for a block only one source has shown.** Such a block is read, but a box proven only there
  is "seen": it is not confirmed, not counted in the balance and not spent until the peers reach that block.
- **On testnet, much less.** Testnet's own difficulty is so low that the floor there is nothing: testnet shows that
  the wallet works, not that a tip is hard to forge.
- **Nodes, for nothing they cannot prove.** A node's REST port is asked for box proofs and to send payments. A
  box it shows must be proven against the agreed tip, so it can hide a box or drop a payment but not invent one.
  It does learn what any node asked learns: the addresses looked up and the payments sent, and so does anyone on
  the path, since most nodes are asked over plain HTTP. A token's name and decimal places are a node's word, and
  are marked as such.
- **Its own age.** The difficulty floor halves every thirty days from the release's checkpoint, so an old build
  is easier to fool on its first run than a new one. `tachys version` says how old the checkpoint is; past sixty
  days, take a newer build (`docs/maintaining.md`).

## Where it sits

The other Ergo wallets we know of (the node's built-in wallet, Satergo, Ergo Mobile Wallet / Terminus, Minotaur,
SAFEW) mostly stand on one of two things: a full node of your own, or a server whose answers are taken as given.
That is our reading of them, not a survey. tachys stands on neither: no node to run, and no answer about your
coins accepted without a proof. It does much less than any of them. Anything that needs knowledge of someone else's contract (swaps,
bonds, a bank) arrives as a signing request from a site built for it, and the wallet's part is to show what the
transaction does and sign (`docs/design.md`).

Two things it lays groundwork for (`docs/roadmap.md`):

- **A true NiPoPoW wallet.** The proof verifier is here and tested (`crates/tachys-verify`). The wallet does not
  choose its tip by it yet, because of what measuring found: today's nodes serve only a stored proof weeks behind
  the tip, and the proof format does not bind a proof's prefix to work.
- **The wallet as a peer.** It already speaks the peer protocol and declares what it is. A prototype message and
  an unmerged node patch (`node-patches/`) give box proofs over that protocol, with no REST port at all; and
  wallets can serve verified headers to each other, relay a proof, and carry a payment.

## Build and test

```
cargo test --workspace                   # about 900 tests, some 840 of them the vendored codec's own; no network
cargo run --release -p tachys -- tips --state state        # learn the tip from mainnet peers
```

`rig/` holds recipes for the peeryard rig (real nodes in network namespaces on one machine): tip sync, payment
by receipt over the peer protocol, a wallet syncing from another wallet, store and forward, a payee with no
route. `.github/workflows/ci.yml` runs the tests and clippy.

| | |
|---|---|
| `crates/tachys-cli` | the wallet: a library with one entry (`tachys::call`) and the `tachys` program |
| `crates/tachys-verify` | checks on a NiPoPoW proof; the rule for choosing a tip by work; the node's difficulty rule |
| `crates/tachys-net` | the smallest peer client that will do: handshake, hear a tip, fetch headers by id, serve |
| `android/` | the phone app |
| `rig/`, `node-patches/` | test recipes; the prototype node change for box proofs over the peer protocol |
| `vendor/arkadianet-codec/` | a copied peer-protocol codec (see its NOTICE.md) |
| `anchors/`, `seeds-*.txt`, `vectors/` | the release checkpoint; peers to try first; proofs and headers the tests read |
| `docs/` | `agents.md`, `json.md`, `design.md`, `android.md`, `good-neighbour.md`, `roadmap.md`, `maintaining.md` |
| `docs/research/` | the findings log (what was tried, measured and changed, in order) and the run logs it cites |

## Licence

tachys is dedicated to the public domain under CC0 1.0 Universal (`LICENSE`), wherever nothing else applies.
What else applies: `vendor/arkadianet-codec/` is other people's work under MIT OR Apache-2.0 and stays under
those terms (its `NOTICE.md` and licence files); `node-patches/` are changes to the Ergo node, itself CC0; and
the crates and libraries tachys is built with keep their own licences. The logo in `img/` (winged sandals, after
Hermes) was drawn for the project by an image model (Gemini) to the maintainer's brief; it and the launcher icons
made from it are offered under the same dedication; `anchors/`, `seeds-*.txt` and `vectors/` hold
public chain data.

Found something wrong, above all something that could lose someone money? `SECURITY.md` says how to report it. For everything else, and for sending a change, see `CONTRIBUTING.md`.
