# Where it stands, and what is next

## Before it should hold real funds

Done:
- Seed phrases can be encrypted at rest: on the command line when a password is given (plain text otherwise, and
  `setup` says so), always in the app, where they open only with the screen lock if the phone has one, or with a
  password of the wallet's own if it was given one (finding 65).
- The tip rule holds a chain to consecutive heights and to the chain's own limits on a change of difficulty.
- The tip is chosen by work, not height, on every path, with a floor that remembers the last agreed tip.
- A payment is never reported sent, failed or confirmed on a node's wording: only on proofs, exit codes and fields.
- Several rounds of review by AI models, some given the code cold, and the fixes that followed
  (`research/findings.md`, 44 to 50).
- Payments made and received on mainnet, from a phone and from the command line, in small amounts.
- A malformed box proof no longer stops the core, in either build: the proof is walked with every bound checked
  before the verifier sees it (finding 52). Measured first, it was worse than this list said: most malformed proofs
  panicked the verifier, and a deeply nested one overflowed the stack of the command-line build too.
- A forged chain has been run against a live client (findings 53 and 55, `tests/forged_chain.rs`), and a first run
  now links the agreed chain back to the release checkpoint, so that a forger must mine every header from it to a
  tip as high as the time since implies: what ten headers at the floor bought before (a fake payment proven to a new
  install) now costs about four hours of the whole network's work with a checkpoint a day old, about a day's with
  one nine days old (arithmetic). A first run on mainnet fetched 1.7 MB in 15 s.
- A payee facing a dishonest payer has been run on the rig (finding 54, `rig/dishonest-payer.sh`): a receipt whose
  payment was double-spent is never accepted, and the same holds after a reorganisation undoes a confirmed one.
  Where the wallet can know a payment is dead it says `void`, and a box whose block left the chain is `undone`,
  instead of "not yet" or "spent"; and a payee given only a box id takes the payment from a node's pool, its body
  checked against the box id and its signatures against its coins, and so tells a late payment from a double-spent
  one (findings 56, 59).
- Bytes from nodes, peers and pages fuzzed through the decoders that read them, from seeds made in the tests (peer
  messages: synthetic); only box proofs ever failed (findings 52 and 57). The JSON of REST answers was not fuzzed.
- The phone screens after the changes (Restore, Receive, Send, consolidation, Sign) run on the Galaxy Note 2 on
  testnet, and two faults found there fixed (finding 58).

Open, in the order they matter:
1. **No person outside the project has reviewed it.** Reviews by models, inside and outside, are recorded in the
   findings (44 to 50, and 59); none is a person's.
2. **What a first run costs, and what it still trusts.** Linking to the checkpoint grows with the checkpoint's age
   (about 0.2 MB a day on mainnet): a release must carry a recent checkpoint (`anchors/`), and that checkpoint is the
   release's word. A wallet that agreed on a tip before finding 55 was not made to link.
3. **A payment dropped from every pool before the payee first asks** cannot be told from one never sent, from a box
   id alone. One confirmation can be undone: the wallet says `undone` only when the block it was confirmed under is
   within the headers it keeps (ten to sixteen: twenty to thirty-two minutes on mainnet); a deeper reorganisation reads
   as `spent`. `undone` has been shown in a test, not on the rig.
4. **The decimal places of a token** not shipped with the app are what a node reported, not something proven.
5. **Nodes are asked over plain HTTP** unless they offer HTTPS: what is asked (addresses) and sent (payments) can
   be read on the way, though not altered without the proofs failing. A node, or anyone on the path, can still
   slow every request down to the two minutes each may take.
6. **A node that is always a block ahead of the peers** gives proofs that settle nothing: boxes stay "seen" and
   payments "waiting" until a sync lands on a block the peers have reached. The only testnet node did this through
   most of our runs (it mines); the app now shows such coins apart instead of as nothing. A proof at a named header (over the
   peer protocol, `node-patches/`) would remove the wait. A sync now also reads a proof at the agreed tip after nodes
   have added headers above it, so such nodes no longer lock out an honest one; `prove_any` (signing requests,
   `wallet receive --pools`, `tachys boxes`) still reads only the newest four stored headers, and takes a proof
   against one above the agreed tip. A Babel offer is read as a sync reads, and only a proof at or below the
   agreed tip is used.
7. **The scalar arithmetic the signing library uses** carries an advisory (RUSTSEC-2026-0387); what was checked
   and what was not is in finding 48.
8. **Testnet has no difficulty floor to speak of** (its checkpoint's difficulty divided by 1024 is nothing), and
   its default quorum is two network groups. Linking to the checkpoint now holds it to the difficulty rule all the way.
9. **The release checkpoint and seed list age.** The difficulty floor halves every thirty days from the checkpoint,
   and a first run fetches more as it ages; a release should carry a recent one (`anchors/`, `seeds-*.txt`).
10. **The app on other phones, and its release build**: the screens were run on the Note 2 with a debug build only;
    not on the Pixel 6 since, not with the camera, not on mainnet.

## Toward a true NiPoPoW wallet

The verifier is here (`tachys-verify`: eleven checks on a proof, the node's difficulty rule ported) and the wallet
does not use it to choose a tip, for two reasons found by measuring (`research/findings.md`, 2 and 4):
nodes today serve only a stored proof from their last snapshot, weeks behind the tip, and the proof format does not
bind a proof's prefix to work. So the tip is learned from the headers peers push, checked against a checkpoint the
release carries: checkpointed SPV. What would change that:

- Node PR #2461 (proof construction and an interlink commitment), followed and not yet merged. It changes the
  proof form; `vectors/pr2461-full-root-proof.bin` and its test hold today's behaviour in place until it is.
- A proof served for the current tip, not the last snapshot.
- A forged proof above the checkpoint, as the direct test of the work and floor checks. (Forged headers are now run
  against the live client, finding 53; a forged NiPoPoW proof is not.)
- Comparing two proofs by accumulated difficulty rather than by how many superblocks each has (Miller et al., CCS
  2025; ergo#2961, its Gap 2): a prototype comparator in `tachys-verify/src/miller.rs`, not used by the wallet.

## Toward the wallet as a peer

A light wallet today is a client of servers. The pieces here make it a small participant in the peer network
instead, which as far as we know is new for Ergo:

- It speaks the peer protocol itself and declares truthfully what it is (`good-neighbour.md`).
- **Box proofs over the peer protocol** (messages 92 and 93): a prototype, and a node patch that is not merged
  (`node-patches/`). With it a wallet needs no REST port at all. Wanted before it is proposed: the header carried
  in the answer, and rate limits on the node's side.
- **Wallets serve each other**: one that has verified a tip passes its headers on (`tachys serve`), relays a proof
  request to its node, and carries a signed payment for a wallet that has no route. Run on the rig
  (`rig/`), not on a public network.
- Next: carry only for known wallets; local discovery of serving wallets; a proof-carrying address lookup over the
  peer protocol; regular syncs that fetch the headers since the stored one in batches, as a first run's linking now
  does (finding 55), instead of only the newest few.

## Smaller things

- A peer that pushes a header heavier than its parent allows is no longer refused as a whole: that header is not
  taken as its tip and the chain under it is judged instead. A warning when a peer pushes such a header would keep the
  signal.
- Rig tests the public networks cannot give: congestion, a lying node; and a shallow reorganisation, where `undone`
  would show (finding 54): JVM nodes need mining on demand, since the rig switches mining only by relaunching a node,
  after which the nodes did not always reconnect. The rig's devnet retargets difficulty its own way, not within
  mainnet's limits: a recipe must keep one miner at a steady rate (a network file's `epoch_length` says 16 for it).
- Accepting a payment by its signed transaction as well as by a box id (most counterparties hand over a transaction
  id; a transaction id alone does not name the coins without a node's word).
- A codec of our own in place of the vendored one, and the binary measured. (Four of its tests go by the clock and
  fail on a loaded machine; `cargo tests` and CI leave them out, and CI runs them alone afterwards.)
- The in-process binding with a key. The app now runs the core in its own process (`crates/tachys-c`, finding 64);
  on the Note 2 that has synced, drawn a QR code beside a sync and renamed a wallet, all with a read-only wallet.
  A seed has been sealed and opened this way with a wallet's own password, and a payment made each way on mainnet
  (finding 65); a signing request has not been run this way on a phone, nor a seed behind the screen lock, nor the
  arm64 library.
- A wallet's own password in the app (finding 65): nothing tests the screens; a wrong password is said at the foot of
  the wallet's settings, which may be off the screen; the app lock and Remove of a screen-lock wallet still ask for
  the phone's lock; a wallet's guard is changed only by restoring it.
- Testnet's difficulty fell to 1 (seen 2026-11-24 UTC), under the floor a wallet that had synced there remembers:
  such a wallet takes no header from testnet's peers. Not looked into further.
- The app on other platforms (`platforms.md`, finding 66). The screens are shared code and run on Linux; Windows and
  macOS are the same code and untried. For an iPhone the shared code is ready (finding 67) and there is a draft of its
  hooks that has never been compiled; there is no app, and nothing has been built for an Apple target. On a computer:
  no installer tried, no camera, and nobody has looked at the window.
- The app's sources are split by what they do, and the wallet's too (finding 71). What the app makes of the core's
  answers is tested for a payment's outcome, a derived address and a wrong password (`vectors/app/`, finding 61), and
  its reading of the wallet file and its own wallet logic against the core (findings 70, 71); what it reads of a
  sync, a receipt and an offer list is not, and one test draws a screen.
- A fee paid in a token from the app (finding 68): paid once on mainnet; the Send screen's keeping the fee back has no
  test; when a node stays a block ahead the app can still only say the offers could not be checked and be asked again.
- The fuzz targets (`fuzz/`: box proofs, peer messages, signed bytes, headers) have run for minutes, not days, and
  not in CI; the JSON of REST answers has no target.
- The fuzzing ran on the build machine; none of it has run in the phone build. A panic there is now caught and
  answered (code 70), as on the build machine; a stack overflow or an abort inside the core would end the app, where
  it used to end only the core's own process (finding 64).
- A box seen only in a block above the agreed tip can still be dropped by a settled proof below where it was seen;
  boxes confirmed under the agreed tip are kept (by reading, not tested).
- A peer whose pushed headers do not follow one another (the reference node pushes tip, tip-16, tip-128, tip-512)
  has its highest taken as its tip, so a lone cheap header among such headers becomes that peer's tip. It only
  decides that peer's own vote.
- `tachys tips` applies the remembered difficulty floor exactly, so it would refuse an honest largest fall in
  difficulty that rounds into the window `extend_chain` allows.
- The proof-of-work hit is computed in `tachys_verify::pow`, not by the library's `Header::check_pow`, which panics on
  a header with a table index of 0 at the sigma-rust revision pinned (sigma-rust issue 964): a peer could make one
  without work and stop `tips`, or abort the phone app. It agrees with the library on every real header in
  `vectors/` and with the reference node on two index-0 headers; once a sigma-rust release carries the fix, it can go
  back to the library. The research module `miller` still reads levels through the library's `max_level_of`, on
  the repository's vectors only.
- One command at a time writes a wallet (finding 70). The chain a state directory keeps (`tips`) is several files,
  each written whole but not as a set: two `tips` at once on one directory are not kept from mixing them. The app
  runs one at a time; the command line does not stop two.
- Every call starts a runtime with a thread for each core and ends it, as each run of the program did. An outside
  reviewer took that for waste on an old phone (finding 70); not measured, so not changed.
- The CI job that builds the phone libraries (`phone-core`, finding 70) was written and has not run.
