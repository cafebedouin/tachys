# Good-neighbour and ban-safety rules

tachys talks to other people's nodes. These rules keep it from being a burden and from being banned.
Bans in the reference node are per IP address, so one misbehaving build can lock out everything else
running from the same address.

## What tachys may send
| message | rule | why |
|---|---|---|
| handshake | session id plus the **truthful light Mode**: digest state, not verifying transactions, NiPoPoW-bootstrapped (so not all headers), keeps no blocks; own agent name | the reference node downloads headers only from peers whose mode says all headers, block sections only from peers whose mode says all blocks, and asks for NiPoPoW proofs from every peer *not* marked NiPoPoW-bootstrapped, which includes peers with no mode. Measured on our devnet: with no mode a proof-bootstrapping node asked us for a proof 10 times in 30 s; with the light mode, never (`docs/research/experiments/2026-11-13-mode-declaration.md`) |
| header request (`RequestModifier`) | by id, few at a time | the handler has no penalty path |
| proof request (`GetNipopowProof`) | m=6, k=10, no header id | anything else is logged and refused; no penalty |
| sync message (`SyncInfo` v2) | **at most one per connection; only a genuine, work-checked header that is behind the network tip** | a header that looks *ahead* of the peer is applied by it, and it then requests that block's sections from us; an invalid header is a misbehaviour penalty. Syncs closer than 100 ms apart are dropped as spam |

## What tachys never sends
- `Inv` for anything except a transaction it holds and will deliver at once when asked (announcing
  data makes the peer request it; not delivering is the non-delivery penalty).
- `Modifiers` except as the answer to that request (unsolicited data is a spam penalty). A node also
  penalises a peer that delivers an **invalid** transaction, which matters for a wallet carrying
  someone else's payment.
- Any header it did not receive from the network and verify.
- The box-proof request (code 92) and the wallet-to-wallet parcel messages (codes 120, 121) to
  public nodes: these exist only in our prototypes. On the rig an unpatched v6.0.8 node ignored
  code 92 without penalty; that is one node version on a private network, not a licence.

## Connection behaviour
- Connect, learn, disconnect. No held slots: a quiet period of about 1.5 s, then one sync message,
  then a few seconds at most.
- Sync when the user asks. The one exception is `tachys wait`, which syncs again every half block until its
  timeout.
- Not built yet: jitter and backoff on failure, and asking for less under congestion.

## Basis and limits
Read in the reference node source (`ErgoNodeViewSynchronizer`, `NetworkController`,
`VersionBasedPeerFilteringRule`): `processSync`, `processSyncV2`, `applyValidContinuationHeaderV2`,
`modifiersReq`, `sendNipopowProof`, `validateAndSetStatus`, `handleHandshake`.
Thresholds as read: non-delivery scores 2 at most once per 2 minutes against a threshold of 500, then
a 60-minute ban.
**Not read:** the 6.1.x reference line and the Rust node implementations, which are many of the peers
on mainnet. Manual bans by operators are outside any reading.
