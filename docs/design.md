# Design: who it is for, the rules it is built by, and where its scope ends

## Who it is for

**The cheapest phone, on paid data, with a connection that comes and goes.** A wallet that needs a server it must
trust, or a connection that stays up, fails this user first. tachys asks the network for little (a sync is about
20 KB from peers and a proof of a few KB), keeps nothing it cannot prove, and treats "no network right now" as a
normal state, not an error.

**Two people face to face, one or both offline.** The payer signs; the payment is a small file or a QR code. The
payee keeps the signed transaction and either of them, or a third wallet carrying it, submits it when the network
is next there. Until it is in a block it is a promise, like a cheque: the payer could still spend the same coins
elsewhere, and the wallet says so in those words. The payee's wallet accepts it only when the box is proven
unspent under a tip the peers agree on. (What a cheque cannot do, guarantee funds while both are offline, needs
coins committed in advance between the two; that is a separate design, not built.)

**A program that needs a wallet.** An agent should not have to learn boxes, fees, change or scripts, or keep a
node, to be paid and to pay. Each thing it wants is one command and one JSON object (`agents.md`), and what it
may spend is set by its owner in a policy file, which binds an agent that runs as a different user from the
wallet's.

## The rules

**Build for the worst case first.** Count what each sync costs the phone (bytes, time) and what it costs the
network (requests, connection-seconds, bytes served), and say both after each sync. Asking for less under
congestion is intended and not built. This is the OXO principle: kitchen tools made for people with limited grip turned out easier for
everyone. If people with low connectivity and few resources can use the wallet, it will work well for people
with better hardware and networks.

**One thing, done well, that other things can build on.** The wallet holds keys, knows what it owns by proof,
pays, receives, and signs. It does those well and exposes them (a signing request in, a signed transaction or a
receipt out) so that more complex things are built from it and not into it.

**Be a good neighbour.** It talks to other people's nodes: it says truthfully what it is, sends as little as it
can, and never sends what would get its address banned (`good-neighbour.md`).

**Say what is proven and what is someone's word.** A balance is boxes proven unspent against a tip the peers
agree on. A token's name, a node's report of a transaction, a block only one source has shown: each is shown as
what it is.

## Scope: what the wallet does itself, and what it leaves to others

The rule: **the line is drawn at what the wallet has to understand.** It does natively what only a key-holder can do and what keeps a wallet healthy.
Anything that needs knowledge of someone else's contract arrives as a signing request from a page
built for it (FreeDEX already builds swaps, the SigmaUSD bank, escrow, bonds and more as static
pages).

| tier | what | why |
|---|---|---|
| Native, core | pay (one or several payees), receive, tokens, consolidation, token burning, pending and offline payments, restore | no third-party contract; consolidation also shrinks proofs and so data use |
| Native, opt-in | fees paid in a token through Babel boxes (EIP-31); possibly holding and paying in SigUSD | they serve the target user directly: someone holding a stable token and no ERG cannot pay a fee today |
| By signing request | swaps, limit orders, liquidity, bonds, escrow, bank mint and redeem, anything else | each needs contract knowledge, extra lookups and upkeep; one feature (ErgoPay: scan or open a request, see it, sign, submit) covers all of them and the other Ergo sites that support it |

For a signing request the wallet's job is one thing: show what the transaction does ("you give X, you
get Y"), computed from the transaction itself and never taken from the page's description, then sign.

## Reasons to keep native contracts few
- Spending a contract box means the Rust library evaluates the script, and it has known differences
  from the node. The usual failure is a rejected transaction, not lost funds.
- Finding a pool or bank box needs an indexed node, and those boxes change every block: more data,
  more failures on poor connections.
- Every native contract is more code in an app that holds keys.

## Offline payments: what they are and are not
A signed payment cannot be altered by the payee or by a wallet carrying it. Until it is in a block
the payer can still spend the same boxes elsewhere, and whichever transaction reaches a block first
wins. An offline receipt is therefore a promise, like a cheque. The wallet accepts a receipt only
when the box is proven unspent. Preventing an offline double spend needs funds committed in advance
between the two parties (a prepaid arrangement, in effect a payment channel); that is a separate
design to prototype, not a wallet feature yet.

## Build order
1. Offline safety: "promised" wording, the payee keeps the signed transaction and can submit it,
   confirmations shown, a pending payment whose coins are gone said to be so (never "failed": the proof cannot
   tell a payment that confirmed and was spent on from one that was overtaken, unless it can prove the first).
2. Several payees, consolidation, token burning.
3. Signing requests, tested against FreeDEX's ErgoPay route.
4. Fees in a token through Babel boxes.
5. Decide on native SigUSD from what gets used.
