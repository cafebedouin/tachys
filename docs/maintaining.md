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
   past sixty days. **A build is good for sixty days from its checkpoint; thirty is better.** To refresh: `./refresh-anchors.sh` does what follows for both networks and leaves the commit to you; by hand, take a header at least a day behind the tip from a node you run or trust
   (`GET /blocks/at/HEIGHT`, then `GET /blocks/ID/header`), check its id on a second source, and replace the file
   with that JSON. `cargo test --workspace` must still pass. No test leans on which header the
   checkpoint is (finding 62: the tests that needed one fixed header read `vectors/header-1914880.json`), so a
   failure after a refresh is to be read, not expected.
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

## Fuzzing

`fuzz/` is a crate of its own (not in the workspace: it wants a nightly toolchain and `cargo install cargo-fuzz`). Its
four targets take the bytes a node or a peer chooses: `box_proof` (the wallet's own reader of a box proof, with no
`catch_unwind`, held to what the honest proof says), `peer_message` (the vendored codec's decoders), `signed_bytes`
(the signing library's transaction, box and signing-request decoders) and `header` (a pushed header and its proof of
work). From the repository's root:

```
cd fuzz && cargo +nightly fuzz run box_proof corpus/box_proof seeds/box_proof -- -max_total_time=600
```

`seeds/` is tracked (an honest input for each target); `corpus/` and `artifacts/` are not. A crash is written to
`fuzz/artifacts/TARGET/`: it is a finding, and its input becomes a test. Run them before a release and after moving
sigma-rust or the codec.

## Changing what programs rely on

- **The answers the app reads** are kept in `vectors/app/`, and held from both sides: the core's tests make each
  again and fail if a field of one is renamed, retyped or gone (`crates/tachys-cli/tests/cli.rs`), and the app's
  tests read them as the app does (`android/app/src/test`, `./gradlew testDebugUnitTest`). If the core's test fails on
  a change that is meant, it prints the new answer: put it in the file, and make the app's tests pass on it.
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

A release is made once a month, and no oftener by plan: a tag on a commit whose tests pass, with a checkpoint
refreshed for it (under a week old). A build is good for sixty days from its checkpoint, so a month between releases
leaves a month in hand; CI's weekly run fails once the checkpoint on the main branch is 45 days old.
What changed is the findings added to `docs/research/findings.md` since the last tag.

1. Refresh the checkpoint (`./refresh-anchors.sh`, which fetches, compares its sources (each network's well-known nodes, and its
   explorer for the id; two that agree are needed) and writes the two files, and commits nothing; `--check` writes
   nothing, and `mainnet` or `testnet` does one network: read what it says and `git diff anchors/`; the rules are above), set the version (`crates/*/Cargo.toml`, the three alike, then any `cargo` command
   to bring `Cargo.lock` along; `versionCode` and `versionName` in `android/app/build.gradle.kts`), commit, push,
   and wait for CI.
2. Tag that commit `vX.Y.Z` and push the tag. `.github/workflows/release.yml` then runs the wallet's tests, builds
   the Linux x86_64 program from the tagged commit (`build-linux.sh`: the path remapping `android/build-core.sh`
   uses, stripped, no build-machine path left in it), refuses if the tag is not the version the program says or the
   checkpoint is a week old, attests the program to that commit and workflow, and opens a **draft** release with the
   program and `SHA256SUMS`. Nothing is public yet.
3. Build the two APKs on your own machine (`android/build-core.sh`, then `./gradlew assembleRelease` with the
   release key; the key and its password are the maintainer's and are nowhere in the repository or on GitHub:
   whoever loses the key can no longer update an installed app). Name them
   `tachys-X.Y.Z-android-arm64-v8a.apk` and `tachys-X.Y.Z-android-armeabi-v7a.apk` (copies in `android/` are
   ignored by git). **With no key named, `assembleRelease` succeeds and signs with the debug key**, and says nothing
   of it (finding 63): before adding them, see that `apksigner verify --print-certs FILE.apk` gives the fingerprint
   in `SECURITY.md` for both. The key can be named for one build without writing its password to a file:
   `ORG_GRADLE_PROJECT_tachysStoreFile=… ORG_GRADLE_PROJECT_tachysStorePassword=… ORG_GRADLE_PROJECT_tachysKeyPassword=… ./gradlew assembleRelease`.
   Then add them to the draft:

   ```
   gh release download vX.Y.Z -p SHA256SUMS
   sha256sum tachys-X.Y.Z-android-*.apk >> SHA256SUMS
   gh release upload vX.Y.Z tachys-X.Y.Z-android-*.apk SHA256SUMS --clobber
   ```

4. Read the draft's notes, check `gh attestation verify tachys-X.Y.Z-linux-x86_64 -R OWNER/REPO` (`gh` 2.49 or
   newer; Ubuntu's own package is older) on the program downloaded from it, then publish: `gh release edit vX.Y.Z --draft=false`.

`./build-linux.sh` builds the same program on any Linux machine (into `target/dist/`); it is not expected to be
the same bytes as the workflow's.

## What comes in from outside

- A report of something that could cost money: `SECURITY.md`.
- Anything else: an issue (the template warns against pasting a wallet file).
- A change: it must pass `cargo test --workspace` and the clippy line in `AGENTS.md`, leave `vendor/` alone, and
  come with a finding if it changes what the wallet does. `anchors/`, the seed lists and the node list are the
  maintainer's to refresh, not a contributor's.
