# Answers for programs: `--json`

Every command takes `--json`. With it:

- **stdout carries exactly one JSON object**, written when the command ends;
- every sentence the command would have printed goes to **stderr** instead (for people and run logs);
- the **exit code** is as without it: 0 done, 1 refused or failed, 2 misuse.

Without `--json` nothing changes. The agent commands (`recipes`, `setup`, `address`, `balance`, `pay`, `accept`,
`wait`, `history`; `docs/agents.md`) always answer with one JSON object of their own, which carries `v`, `ok` and
its own fields (and `code` and `refused` when refused) but not `exit` or `command`, and are built on these answers.

A caller decides on the exit code and the fields, never on the wording of a sentence. Sentences may quote
what a node said. So may `error`, and the fields named for it (`node_says`, a token's `name` and `decimals`); no
other field does. The app and the agent commands read no sentence to decide anything.

If a command panics (a fault in tachys), the exit code is 70 and `error` says so.

## Every answer

| field | |
|---|---|
| `v` | the version of these answers, now `1`. It changes if a field is renamed, retyped or removed; a new field does not change it. A caller should refuse a `v` it does not know. |
| `ok` | `true` exactly when the exit code is 0 |
| `exit` | the exit code |
| `command` | the command, e.g. `"wallet send"` |
| `error` | only when `ok` is false: the reason, in words |

## By command

**`tips`** — `verdict`: `"agreed"` or `"not_trusted"`. When agreed: `tip` (`height`, `id`, `timestamp_ms`, `age_s`,
`state_root`), `groups` (network groups that have reached it), `ahead` (headers kept above it that fewer groups
have reached), `saved`. Unless no peer was heard at all: `peers_heard`, `refusals`. Always: `checkpoint_age_days`
(how old this build's checkpoint is; past sixty, take a newer build).

**`wallet status | receive | restore`** — `address`, `addresses`, `nanoerg`, `boxes` (how many), `tokens`
(`id`, `amount`), `confirmed_nanoerg` (the part of `nanoerg` in boxes confirmed under an agreed tip: what can be
spent), `proven_at` (the height of the proof), `agreed_height`, `settled` (the proof is at or below the
agreed tip), `expecting`, `cost`. `checked` lists each box asked about: `id`, `height`, and `state`:

| `state` | |
|---|---|
| `proven` | unspent, pays this wallet, and confirmed: proven at or below a tip the peers agreed on, now or on an earlier sync; with `nanoerg`, `confirmations` (a lower bound), `added` |
| `seen` | unspent and pays this wallet, but proven only in a block above the agreed tip: one source, not confirmed, not spent |
| `absent` | no such unspent box |
| `spent` | a box this wallet tracked is gone (removed), or a receipt asked about that a payment of this wallet has since spent |
| `expected` | a box this wallet expects from its own payment, not on chain yet |
| `other_address` | it exists and pays someone else |
| `undecodable` | present, but not a box this build can read |
| `absent_above_agreed_tip` | a tracked box absent in a block the peers have not reached: kept |
| `absent_below_confirmed` | a tracked box absent in a block below the one it was confirmed at (the node is behind; the box did not exist yet there): kept |

`payments` lists what became of payments sent: `tx` and `result` (`waiting`, `confirmed`, `coins spent`,
`not included`). A payment proven to be in a block though its boxes are gone is `confirmed` with `height` and
`proof: "inclusion"`. `coins spent`: its coins are gone and nothing it created is unspent, so the proof cannot say
whether it confirmed (and its boxes were spent since, by the payee or by another wallet of the same phrase) or was
overtaken; `node_says` then carries what a node reports, as its word. `refresh_due`: something else has moved this
wallet's coins (another wallet of the phrase), so the next `status` asks the nodes again what its addresses hold.

**`wallet send | consolidate | burn | babel-offer`** — `signed`, `leaves_wallet_nanoerg` (what the wallet's boxes
put in less what comes back, fee included: the number the spending policy is held to), `fee_nanoerg`, `tokens_out`,
`payees`. Once signed: `tx`, `receipt` (the payee's box ids), `change`. If a node took it: `broadcast`
(`node`, `accepted: true`); there is no `broadcast` field otherwise. `outcome_unknown: true` (with exit 1): a node
was sent the payment and did not answer. It may have gone out; it is kept as waiting with its coins set aside, and
the next `status` settles it. It must not be paid again on a guess. `fee_token`, `fee_token_units`: a fee paid in a token. With `--parcel-out`: `parcel`, `tx_bytes`.
With `--request-out`: `request`. **`--dry-run`** builds the same transaction, asks the policy, and stops before
signing: the same fields with `signed: false`, and nothing written or counted.

**`wallet request`** — `give`, `get`, `others_put_in`, `others_receive` (each `nanoerg` and `tokens`), `fee_nanoerg`,
`net_nanoerg`, `paid_to`, `inputs`, `signed`; when signed and sent, `tx` and `sent_via`.

**`wallet babel-list`** — `offers`: `box`, `available_nanoerg`, `nanoerg_per_unit`.

`listed_by_all` (with `--reconcile`): every node asked answered for every address. `refresh_due`, `node_says`:
see below.

**`wallet new | restore | watch | derive`** — `address`; `watch` adds `watch`; `name` gives `name`; `encrypt` gives `sealed`; `new` adds `birthday`, `sealed`; `restore` adds `restored`
(`new`, `upgraded`, `resealed`). **`wallet address`** — `addresses`, and `added` with `--add`. **`wallet pubkey`** —
`hex`, `base58`. **`wallet promise`** — `promised` (box ids), `nanoerg`, `tokens`. **`wallet mnemonic`** — `mnemonic`
(the command exists to show it; it is refused under a spending policy).

**`boxes`** — `proven_at`, `unspent`, `nanoerg`, and `checked` (`id`, `height`, `state`: `unspent`, `absent`, `undecodable`).
**`nodes`** — `nodes`: `url`, `height`, `ms`, `well_known`, `used`. **`version`** — `version`, `checkpoint_height`,
`checkpoint_age_days`, `network`. **`token`** — `id`, `name`, `decimals`, `reported_by`.
**`outbox`** — `sent`, `waiting`, `dropped` (or `submitted`, `refused` over the peer protocol). **`handoff`** — `acknowledged`.
**`qr`** — `rows`. **`sync`, `check`** — `proofs_verdict` (`agreed` or `not_trusted`), `tip`, `refusals`.
**`included --tx TXID`** — proves a transaction is in the agreed chain: `included`, `height`, `confirmations`,
`agreed_height`. Exit 1 with `included: false` means it could not be shown, not that it is absent.

**Any wallet command given a password** — `bad_password: true` when it does not open the seed phrase. A command that
needs the phrase (signing, `mnemonic`, `pubkey`, `address --add`, a `--dry-run` payment) is then refused; one that
does not (`status`, `receive`, `address`, …) goes on with the phrase closed, as with no password.

## How a command ends

No command ends the process. Each hands back how it went (an exit code and the reason) and the program's `main`
alone exits, after writing the object.

## From Rust, in the same process

```rust
let a: tachys::Answer = tachys::call(&["tachys".into(), "balance".into(), "--state".into(), dir], &[]);
// a.code: 0, 1 or 2 (70: the command panicked); a.out: the one JSON object; a.err: the sentences
```

`args` are as on the command line, the program's name first. `input` are the lines a command reads where the
program would read stdin: the password for `--pass-stdin`, then a seed phrase for `--mnemonic-file -`. Each call
chooses its own network and leaves nothing for the next; nothing is written to the process's stdout; calls are
taken one at a time. Do not call it from inside an async runtime: it runs its own.

## What may change

`v` is 1. Within a `v`, a field may be added; none is renamed, retyped or removed, and a `code` keeps its meaning.
A test holds the field names of the agent commands' answers in place (`crates/tachys-cli/tests/cli.rs`). Command
names and options are held to the same rule from the first tagged release; before it, this project is an
experiment and they may still move, with a finding in the log when they do. Files in a state directory other than
`wallet.json` (the stored chain, the node list, the payments log) are the wallet's own and carry no promise.

## The wallet file

`<state>/wallet.json` carries `"version": 1` (`crates/tachys-cli/src/walletfile.rs` names every field and its type).
A file with no version is read as it is and gets the number when next written. A file with a higher version, or a
field of the wrong type, is refused with the reason and left untouched; the app says so on its first screen
instead of leaving the wallet out.
