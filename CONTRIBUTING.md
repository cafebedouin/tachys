# Contributing

tachys is experimental and small. Help is welcome: a report that something is wrong, a fix, a test, a clearer
sentence. This page says how.

## Reporting a problem

- **Something that could cost someone money**: not in public. `SECURITY.md` says how.
- **Anything else**: open an issue. Say what you ran, what you expected and what happened; `tachys version` and
  the answer of the command with `--json` are usually enough. Never paste a wallet file, a seed phrase or a state
  directory: a wallet file can hold the seed phrase in plain text.
- Before a large change, open an issue first and say what you mean to do. It saves work that would not be taken.

## Making a change

1. Fork the repository and work on a branch of your own.
2. Build and test. Neither needs a network:

   ```
   cargo tests
   cargo clippy -p tachys -p tachys-verify -p tachys-net -p tachys-c --all-targets -- -D warnings
   ```

   Both must be clean. (`cargo tests` is `cargo test --workspace` without four of the vendored codec's tests that go
   by the clock and fail on a busy machine: `.cargo/config.toml`.) CI runs the same, with the phone build and `cargo audit`.
3. Keep to how the code is written (`AGENTS.md`, "If you are working on this repository", is the full list):
   - formatted with rustfmt, which CI checks: `cargo fmt -p tachys -p tachys-verify -p tachys-net -p tachys-c`
     (never a bare `cargo fmt`: it would rewrite `vendor/`);
   - a command never ends the process, prints through `say!`/`complain!`, and reports through fields: a caller
     must never have to read a sentence to decide anything;
   - tests are named as sentences, need no network, and write only under the system's temporary directory;
   - `vendor/` is other people's work and is left as upstream wrote it.
4. If the change alters what the wallet does, add a numbered finding at the end of `docs/research/findings.md`:
   what was wrong, what changed, then **Tests**, **Run** and **Not run**. Say plainly what is proven and what is
   someone's word. If it changes an answer programs rely on, `docs/maintaining.md` ("Changing what programs rely
   on") applies, and `docs/json.md` changes with it.
5. Open a pull request against `main`. One subject per pull request; say what it does and how you checked it.
   Small ones are read sooner.

## What is not taken from outside

`anchors/`, the seed lists, the well-known node list and the app's token names are what the wallet trusts before
it has heard from anyone. They are refreshed by the maintainer alone (`docs/maintaining.md`). If one is wrong or
stale, open an issue.

## Licence of what you send

The project is dedicated to the public domain under CC0 1.0 (`LICENSE`). By sending a contribution you agree that
it is offered under the same dedication, and that it is yours to offer. There is no contributor agreement to sign.

## Conduct

Be civil, argue about the code and the evidence, and assume the other person means well. The maintainer may close
or remove what does not.

## Agents

A change written with or by a model is welcome on the same terms: it is tested, it says what was and was not run,
and a person answers for it. `AGENTS.md` is written for you.
