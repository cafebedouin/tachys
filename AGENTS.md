# For agents

## If you need a wallet

Build once (`cargo build --release -p tachys`), then run `target/release/tachys recipes`. It prints one JSON object
(about 4 KB) that names, for each thing you might want, the one command to run and what it gives back. You do not need
to know how the chain works, and you do not need a node.

The rules that matter:

- Each of these commands prints one JSON object and exits 0 (done), 1 (refused, failed, or not yet so), 2 (misuse).
  `ok` is true exactly when the exit code is 0. Otherwise `refused` says why in words and `code` is a word to
  branch on: `policy` will not pass by retrying; `no_tip`, `no_node`, `not_proven` may pass later; `not_yet` means
  ask again after a block.
- The default network is mainnet: real money. Add `--network testnet` for testnet, on every command.
- Use one state directory per wallet, the same one every time (`--state DIR`).
- `pay` works only if the wallet's owner wrote `DIR/policy.json` and an empty `DIR/payments.log`. A refusal names
  the limit. If you run under another account than the wallet's, you cannot change the limits.
- `pay` with `ok: true` means a node took the payment, not that it is final. `tachys wait --tx TX` tells you when it is.
- If `pay` answers `outcome_unknown: true`, do not pay again. Run `tachys wait --tx TX`.
- To be paid: give the payer `address` from `tachys setup`; ask them for the receipt (a box id, not a transaction
  id); then `tachys wait --receipt BOXID`. `accepted` says the box is real, unspent and yours. Check for yourself
  that `receipt.nanoerg` (and `receipt.tokens`) is what you were owed and that `receipt.first_seen` is true: a
  box you have been shown before is not a new payment.
- `setup` answers `sealed: false` when the seed phrase is in plain text in `DIR`; it is sealed only if
  `TACHYS_PASS` was set when the wallet was made.
- Amounts are nanoERG (1 ERG = 1000000000) unless a field says `erg`. A payment costs a fee of 1100000 nanoERG.

More: `docs/agents.md`. The fields of every answer: `docs/json.md`.

## If you are working on this repository

- `cargo test --workspace` must pass (no network needed; four of the vendored codec's tests go by the clock and fail
  on a busy machine, so if only those fail, run them alone: `cargo test -p ergo-p2p --lib`), and
  `cargo clippy -p tachys -p tachys-verify -p tachys-net -p tachys-c --all-targets -- -D warnings` must be clean. The
  vendored codec under `vendor/` is kept as upstream wrote it: do not reformat or lint it.
- The four tachys crates are rustfmt-formatted, and CI checks it: run
  `cargo fmt -p tachys -p tachys-verify -p tachys-net -p tachys-c` before a commit. Never a bare `cargo fmt`, which
  would rewrite `vendor/` too.
- A command never ends the process, prints through `say!`/`complain!` (never `println!`), and reports through
  fields (`out::set`). A caller must never have to read a sentence to decide anything.
- A new option is known as soon as some command reads it (`build.rs` gathers them); there is no list to update.
- What changed and why is logged as a numbered finding at the end of `docs/research/findings.md`: what was wrong,
  what changed, then **Tests**, **Run** and **Not run**. A run worth citing gets a log in `docs/research/runs/`
  named `DATE-what.txt`. A change with no effect on behaviour needs no finding. Say plainly what is proven and
  what is someone's word.
- Tests are named as sentences, need no network, and write only under the system's temporary directory.
- Contributions are taken under the project's dedication (CC0); `vendor/` is not ours to change. `anchors/`, the
  seed lists and the node list are refreshed by the maintainer alone (`docs/maintaining.md`).
- The app is `android/`: its screens and wallet logic once in `android/shared/src/commonMain` (Compose Multiplatform; nothing of the JVM or
  of any one system may be used there, and `./gradlew :shared:compileCommonMainKotlinMetadata` says if it is), the Android
  app around them in `android/app`, and the same screens in a window on a computer (`docs/platforms.md`). Its tests are
  `./gradlew :shared:desktopTest`. After a change to the core run `android/build-core.sh` before building for a phone. The app
  runs the core in its own process through `crates/tachys-c` (a C interface, `tachys.h`, and the JNI entry): a
  command may run beside another, so nothing a command sets may outlive it or be seen by another (`own.rs`).
- Wallet state directories, seed phrases and anything else private live in `private/` or `state*/`, both ignored.
  Never commit one.
