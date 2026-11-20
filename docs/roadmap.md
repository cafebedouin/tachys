# Where it stands, and what is next

## Before it should hold real funds

Done:
- Seed phrases can be encrypted at rest: on the command line when a password is given (plain text otherwise, and
  `setup` says so), always in the app, where they open only with the screen lock if the phone has one.
- The tip rule holds a chain to consecutive heights and to the chain's own limits on a change of difficulty.
- The tip is chosen by work, not height, on every path, with a floor that remembers the last agreed tip.
- A payment is never reported sent, failed or confirmed on a node's wording: only on proofs, exit codes and fields.
- Several rounds of review by AI models, some given the code cold, and the fixes that followed
  (`research/findings.md`, 44 to 50).
- Payments made and received on mainnet, from a phone and from the command line, in small amounts.

Open, in the order they matter:
1. **No person outside the project has reviewed it.**
2. **The first run of a new install** has only the release's checkpoint and difficulty floor to judge a tip by. With
   every connection in an attacker's hands, a false tip costs about ten headers at that floor. No forged chain has
   been run against a live client.
3. **A malformed box proof can still stop the core** where the build aborts on a panic (the phone build). The app
   survives because the core is a separate process; an in-process binding must not be made before this is closed.
4. **The decimal places of a token** not shipped with the app are what a node reported, not something proven.
5. **Nodes are asked over plain HTTP** unless they offer HTTPS: what is asked (addresses) and sent (payments) can
   be read on the way, though not altered without the proofs failing.
6. **The phone screens after the last changes**: Send, Sign, Restore and consolidation have not been run on a
   phone since.
7. **A restored wallet keeps only its first address if all twenty are empty when it is looked over**; a payment
   made later to another of them is not seen.
8. **The scalar arithmetic the signing library uses** carries an advisory (RUSTSEC-2026-0387); what was checked
   and what was not is in finding 48.
9. **A node that is always a block ahead of the peers** gives proofs that settle nothing: boxes stay "seen" and
   payments "waiting" until a sync lands on a block the peers have reached. In practice the next sync does; a
   proof at a named header (over the peer protocol, `node-patches/`) would remove the wait. A sync now also reads a
   proof at the agreed tip after nodes have added headers above it, so such nodes no longer lock out an honest one;
   `prove_any` (signing requests, offers, `tachys boxes`) still reads only the newest four stored headers.
10. **Testnet has no difficulty floor to speak of** (its checkpoint's difficulty divided by 1024 is nothing), and
    its default quorum is two network groups.
11. **The release checkpoint and seed list age.** The difficulty floor halves every thirty days from the checkpoint;
   a release should carry a recent one (`anchors/`, `seeds-*.txt`).

## Toward a true NiPoPoW wallet

The verifier is here (`tachys-verify`: eleven checks on a proof, the node's difficulty rule ported) and the wallet
does not use it to choose a tip, for two reasons found by measuring (`research/findings.md`, 2 and 4):
nodes today serve only a stored proof from their last snapshot, weeks behind the tip, and the proof format does not
bind a proof's prefix to work. So the tip is learned from the headers peers push, checked against a checkpoint the
release carries: checkpointed SPV. What would change that:

- Node PR #2461 (proof construction and an interlink commitment), followed and not yet merged. It changes the
  proof form; `vectors/pr2461-full-root-proof.bin` and its test hold today's behaviour in place until it is.
- A proof served for the current tip, not the last snapshot.
- A forged proof above the checkpoint, as the direct test of the work and floor checks.

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
  peer protocol; header sync in one batched request from the stored header.

## Smaller things

- Rig tests the public networks cannot give: congestion, a lying node, a double-spent receipt.
- A codec of our own in place of the vendored one, and the binary measured.
- An in-process binding for the app (the library entry `tachys::call` is ready for one) once item 3 above is closed.
- The command that builds payments (`wallet send`) and the app's main source file are each too long.
- Accepting a payment by its transaction id as well as by a box id: most counterparties will hand over the first.
- A box seen only in a block above the agreed tip can still be dropped by a settled proof below where it was seen;
  boxes confirmed under the agreed tip are kept (by reading, not tested).
- A peer whose pushed headers do not follow one another (the reference node pushes tip, tip-16, tip-128, tip-512)
  has its highest taken as its tip, so a lone cheap header among such headers becomes that peer's tip. It only
  decides that peer's own vote.
- `tachys tips` applies the remembered difficulty floor exactly, so it would refuse an honest largest fall in
  difficulty that rounds into the window `extend_chain` allows.
- `balance` formats `erg` through a float and loses digits above about nine million ERG; `nanoerg` is exact.
