# Reporting a problem that could cost someone money

tachys is an experiment kept by one unpaid maintainer. It holds keys and moves coins, so a flaw in it matters.

**Report it privately**, through this repository's "Report a vulnerability" button (Security tab, private
vulnerability reporting). Please do not open a public issue for something that could be used to take or lose
coins, forge a balance, or get a false chain tip accepted. Expect an answer within a week.

**Never send a seed phrase, a `wallet.json`, a password, `TACHYS_PASS`, or a whole state directory**, to anyone,
in a report or anywhere else. A wallet file can hold the seed phrase in plain text. What a report needs: the
commit, the network, the command you ran, its JSON answer and exit code.

What is supported: the newest commit on the main branch, built from source. No binaries are published by this
project; one you were given is someone else's build.

Known and accepted, so not news: RUSTSEC-2026-0387 (`crypto-bigint`, through the signing library; what was checked
is in `.cargo/audit.toml` and finding 48 of `docs/research/findings.md`), and the open items listed in
`docs/roadmap.md` under "Before it should hold real funds".
