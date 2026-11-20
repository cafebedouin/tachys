# Good-neighbour and ban-safety rules

tachys talks to other people's nodes. These rules keep it from being a burden and from being banned.
Bans in the reference node are per IP address, so one misbehaving build can lock out everything else
running from the same address.

## What tachys may send
| message | rule | why |
|---|---|---|
| handshake | session id plus the **truthful light Mode**: digest state, not verifying transactions, NiPoPoW-bootstrapped (so not all headers), keeps no blocks; own agent name | the reference node downloads headers only from peers whose mode says all headers, block sections only from peers whose mode says all blocks, and asks for NiPoPoW proofs from every peer *not* marked NiPoPoW-bootstrapped, which includes peers with no mode. Measured on our devnet: with no mode a proof-bootstrapping node asked us for a proof 10 times in 30 s; with the light mode, never (`docs/research/experiments/2026-11-13-mode-declaration.md`) |
| header request (`RequestModifier`) | by id: a few at a time, except a first run linking to the checkpoint, which asks in one message for the ids the peer has just announced to it (up to 400) | the handler has no penalty path and answers a request of any size in parts (`modifiersReq`, `sendByParts`) |
| proof request (`GetNipopowProof`) | m=6, k=10, no header id | anything else is logged and refused; no penalty |
| sync message (`SyncInfo` v2) | **only a genuine, work-checked header that is behind the peer's tip.** At most one per connection, except a first run linking to the checkpoint (`tips`, finding 55): then one a second at most, each offering the newest header the same peer has just sent us, so always one it holds and is past | a header that looks *ahead* of the peer is applied by it, and it then requests that block's sections from us; an invalid header is a misbehaviour penalty. Syncs closer than 100 ms apart are dropped as spam, without a penalty (`processSync`, `PerPeerSyncLockTime`). To a peer behind it the node answers with up to 400 ids that follow (`continuationIds(..., size = 400)`, v1 and v2 alike) |

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
- The exception is a first run, which links to the release checkpoint: one connection to one peer, rounds of one
  sync message and one request a second at most, until the headers between are in. Measured: 15 s for 5,833 mainnet
  headers; 209 s for 11,125 testnet headers from a slow peer. A peer has five minutes of it at most, and the whole
  link fifteen; then the next peer is asked, from where the last left off. It grows with the age of the release's
  checkpoint (`maintaining.md`). A wallet that keeps the chain to serve others (`tips --keep-chain`) asks only for
  what is new since its last run.
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

Assumptions the code makes about nodes, read in v6.0.6 and held by nothing but this list: a node announces up to 400
ids after a header offered to it; it answers a header request in one or more `Modifiers` messages within ten seconds;
it serves `/transactions/unconfirmed/outputs/byBoxId/{id}` and `/transactions/unconfirmed/byTransactionId/{id}` (used by
`wallet receive --pools`). If a later node version changes any of these, a first run or `--pools` will find less,
and say so; nothing is taken on a node's word because of them.
