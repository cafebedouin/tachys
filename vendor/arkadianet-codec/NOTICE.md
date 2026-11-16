# Vendored P2P codec

`ergo-primitives`, `ergo-ser` and `ergo-p2p` are copied unchanged from
https://github.com/arkadianet/ergo at commit 7670c3be8a5ed57d6c50ee84f9477b026cdcc581 (2026-09-07),
licensed MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE in this directory).

Checked 2026-11-14 against that commit: the repository is public, its workspace licence is
`MIT OR Apache-2.0`, the two licence files here are byte-identical to its own, and every file under
the three crates is identical. Not copied: each crate's `tests/` directory.

`test-vectors/` holds the five fixture files that the crates' unit tests read, copied on 2026-11-15 from the
same repository at the same commit (`test-vectors/` there) under the same licence: one for `ergo-primitives`
and four for `ergo-ser`. Without them six unit tests failed for want of a file; with them
`cargo test --workspace` passes. The rest of the repository's `test-vectors/` is not copied.

They are here so the proof of concept can speak the Ergo peer protocol from day one. tachys uses only
framing, the handshake and the NiPoPoW request/response codecs; replacing this with a minimal codec
of our own is a planned step, measured against the footprint goal.
