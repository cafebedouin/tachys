# Keeping it up

What goes stale by itself, what to do about it, and how to change things without breaking the programs that
call this one. Written for one person.

## Once a month, and before anything is tagged

1. **The checkpoint** (`anchors/mainnet.json`, `anchors/testnet.json`). A tip is accepted above a difficulty floor
   that starts at a quarter of the checkpoint's difficulty (a 1024th on testnet) and halves every thirty days,
   with no lower bound. A fresh install also links its first chain back to the checkpoint (finding 55): every header
   since it, a step the difficulty rule allows, up to at least half the height the time since implies. So a false
   first tip costs every one of those headers at no less than the floor (about four hours of the whole network's
   work with a checkpoint a day old, about a day's at nine days), and an honest first run fetches them all: about
   0.2 MB and 1.8 s a day of the checkpoint's age on mainnet (1.7 MB at nine days; about 12 MB and two minutes at
   sixty), more on testnet (3.7 MB at seven days). The floor's halving makes an old checkpoint weaker, and the
   download makes it dearer to start from: a release should carry one a few days old. Nothing breaks when a
   checkpoint is old: `tachys version` and every `tips` answer say its age (`checkpoint_age_days`), and `tips` warns
   past sixty days. **A build is good for sixty days from its checkpoint; thirty is better.** To refresh: take a header at least a day behind the tip from a node you run or trust
   (`GET /blocks/at/HEIGHT`, then `GET /blocks/ID/header`), check its id on a second source, and replace the file
   with that JSON. `cargo test --workspace` must still pass (the test vectors are above the old checkpoint; if the
   new one is above them, the vectors need refreshing too: `crates/tachys-verify/tests/`).
2. **The seed lists** (`seeds-mainnet.txt`, `seeds-testnet.txt`, and the copies the app ships in
   `android/app/src/main/assets/`, which must be identical: CI compares them). Only the first sixteen to
   twenty-four lines are tried. Put peers that answer at the top; `tachys sync --save DIR` shows who does.
3. **The well-known nodes** (`crates/tachys-cli/src/nodes.rs`, `well_known`). They are other people's. Check each
   still answers (`tachys nodes --refresh`), and that testnet still has one.
4. **`cargo audit`**. A new advisory is either fixed, or written into `.cargo/audit.toml` with the reason.
5. **The protocol version sent in the handshake** (`crates/tachys-net/src/lib.rs`, `PROTOCOL`). If nodes begin
   to turn older peers away, the symptom is "no peer pushed a usable tip".
6. **The token names the app ships** (`android/app/src/main/assets/tokens.json`: id, name, decimals; mainnet). The
   rule is the one FreeDEX uses for its swap list: a token is named if its deepest ErgoDEX v1 pool against ERG
   holds 100 ERG or more on the day the list is made, and the name and decimals are the ones that pool's token
   was issued with. The list was last made on 2026-11-16 (55 tokens) with FreeDEX's `scripts/tokens.ts`. A token
   not on it is shown by what a node says of it, marked as the node's word.

## Dependencies

- **sigma-rust** is pinned by revision in `crates/*/Cargo.toml` (the same revision everywhere). To move it: change
  every occurrence, `cargo update`, run the tests and the four rig recipes, and read its changelog for anything
  touching signing, box selection or the AVL verifier.
- **The vendored codec** (`vendor/arkadianet-codec/`) is a copy; its `NOTICE.md` records the upstream commit. It is
  not edited here. To take an upstream fix, copy the files again from a named commit and update the notice.
- **`Cargo.lock` is tracked** and CI builds with `--locked`.
- **The Android side** pins its plugin, Kotlin and Compose versions in `android/build.gradle.kts` and
  `android/app/build.gradle.kts`. After moving any, build and try the app on the oldest and the newest phone to
  hand (`./gradlew assembleRelease -PtachysIdSuffix=.test` installs a copy beside the real one).
- **The launcher icons** are generated from `img/tachys-logo.jpg` by `android/make-icons.py` and checked in.

## Changing what programs rely on

- **JSON answers** carry `v` (now 1). Adding a field is free. Renaming, retyping or removing one, or changing what
  a `code` means, is a new `v`: say so in a finding, change `out::V`, and keep the test that holds the agent
  answers' field names (`crates/tachys-cli/tests/cli.rs`) in step.
- **`wallet.json`** carries `version` (now 1; `walletfile.rs`). A new layout gets a new number and code that
  reads the old one. A build never writes a file an older build would misread without refusing.
- **Commands and options**: until the first tag they may move, with a finding. After it, a removed or renamed one
  is a breaking change, said in the tag's notes.
- **`tachys recipes`** is a document agents read. A recipe must name only commands and options that exist; a test
  runs each option it mentions.

## Releasing

A release is a tag on a commit whose tests pass, with a checkpoint under a week old. It carries, built from that
commit: the two APKs (`android/build-core.sh`, then `./gradlew assembleRelease` with the release key; the key and
its password are the maintainer's and are nowhere in the repository: whoever loses the key can no longer update
an installed app), the Linux x86_64 program (`cargo build --release --locked -p tachys` with the path remapping
`android/build-core.sh` uses, stripped; it needs glibc 2.34 or newer), and `SHA256SUMS` over all of them. Check
that `strings` finds no build-machine path in any of them. Tag the publication `v0.0.1`; tag again at each checkpoint refresh. What changed is the findings
added to `docs/research/findings.md` since the last tag.

## What comes in from outside

- A report of something that could cost money: `SECURITY.md`.
- Anything else: an issue (the template warns against pasting a wallet file).
- A change: it must pass `cargo test --workspace` and the clippy line in `AGENTS.md`, leave `vendor/` alone, and
  come with a finding if it changes what the wallet does. `anchors/`, the seed lists and the node list are the
  maintainer's to refresh, not a contributor's.
