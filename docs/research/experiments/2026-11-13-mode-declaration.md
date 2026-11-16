# What a declared mode changes about what a node asks of us

Written before the run.

**Read in the reference node (v6.0.8 line):** a peer's Mode feature is consulted in three places.
Headers are downloaded only from peers whose mode exists and is not NiPoPoW-bootstrapped. Block
sections only from peers whose mode exists and keeps all blocks. NiPoPoW proofs are requested from
peers that are not NiPoPoW-bootstrapped, which includes peers with no mode at all.

**Setup:** node A mines on the private devnet. For each arm a fresh node B (not mining) starts with
A as its only known peer and syncs from scratch. tachys connects to B, sends nothing after the
handshake, and counts the frames B sends it for 25 s.

**Arms and predictions**
| arm | handshake declares | RequestModifier to us | GetNipopowProof to us |
|---|---|---|---|
| nothing | no Mode feature | 0 | 0 (B is not bootstrapping by proof) |
| light | digest, not verifying, NiPoPoW-bootstrapped, 0 blocks | 0 | 0 |
| full-claim (control) | archival: utxo, verifying, all blocks | more than 0, if B picks us while syncing |

**What counts against the reading:** a request in the nothing or light arm. **What makes the run
uninformative:** zero in the control arm too (then the instrument cannot see a request at all, and
the zeros in the other arms mean nothing).

## Results (2026-11-13)

**Run 1, B syncing normally** (`docs/research/runs/2026-11-13-mode-declaration.txt`; two earlier attempts are
kept and marked void and partial): B synced about 370 blocks from A in every arm and asked tachys for
nothing in any arm, the control included. By the rule stated above the run cannot tell the
declarations apart. The reason, from the code: a syncing node asks only peers it believes are ahead of
it, and tachys sent no sync message, so its declared mode was never consulted. What the run does show:
a client that never claims to be ahead is asked for nothing whatever it declares, and a syncing node
sends every connected peer about 1,500 inventory frames (50 KB) in 30 s.

**Run 2, B bootstrapping from a NiPoPoW proof** (`docs/research/runs/2026-11-13-mode-declaration-nipopow.txt`):

| arm | GetNipopowProof to us in 30 s |
|---|---|
| nothing | 10 |
| light | 0 |
| full-claim (control) | 10 |

This is the one place where the code treats "no mode" and "light" differently, and the measurement
agrees with it. A wallet that declares nothing is treated as a possible proof provider.

**Decision:** tachys declares the light mode by default. A mainnet tip sync with it behaved as before
(five network groups agreed).

**Not covered:** the 6.1.x reference line and the Rust nodes; how a peer that claims to be *ahead* is
treated (that path is not gated by mode and tachys never takes it).
