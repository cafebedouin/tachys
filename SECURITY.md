# Reporting a problem that could cost someone money

tachys is an experiment kept by one unpaid maintainer. It holds keys and moves coins, so a flaw in it matters.

**Report it privately**, through this repository's "Report a vulnerability" button (Security tab, private
vulnerability reporting). Please do not open a public issue for something that could be used to take or lose
coins, forge a balance, or get a false chain tip accepted. Expect an answer within a week.

**Never send a seed phrase, a `wallet.json`, a password, `TACHYS_PASS`, or a whole state directory**, to anyone,
in a report or anywhere else. A wallet file can hold the seed phrase in plain text. What a report needs: the
commit, the network, the command you ran, its JSON answer and exit code.

What is supported: the newest commit on the main branch, built from source, and the newest release on this
repository's Releases page. A release carries the Linux program, two APKs and `SHA256SUMS`. From v0.0.2 on the
Linux program is built by this repository's release workflow from the tagged commit, and attested to it:
`gh attestation verify FILE -R cafebedouin/tachys` shows which commit and workflow made a download (that is
GitHub's word for its own runner, and not a reproducible build). The APKs are built by the maintainer on their own
machine, with no attestation: for them, and for everything in v0.0.1, the checksums show that a download is what
was uploaded here, not what source it came from. The APKs are signed with the project's key; its certificate's
SHA-256 is

    6501129cab8edd4a7ea74a693ba77c16e1b1799ff966c191d794c3719d20ff7b

(`apksigner verify --print-certs FILE.apk` shows it). A binary from anywhere else is someone else's build.

Known and accepted, so not news: RUSTSEC-2026-0387 (`crypto-bigint`, through the signing library; what was checked
is in `.cargo/audit.toml` and finding 48 of `docs/research/findings.md`), and the open items listed in
`docs/roadmap.md` under "Before it should hold real funds".
