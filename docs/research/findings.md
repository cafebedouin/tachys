# What was learned, in the order it was learned

This is the project's log: each finding is what was tried, what was measured and what changed, with the run log
it rests on (`runs/`). It is long and it is evidence, not an introduction; start at the README. Findings are
numbered and never renumbered, because the code's comments cite them. Later findings correct earlier ones; where
they disagree, the later one is how the code stands. Commit ids and the branch name some entries cite belong to the
project's history before publication, which is not part of this repository. Wallet addresses are left out and the
ids of boxes and transactions are cut short: they are a private test wallet's, and the logs' point is what was
measured, not whose coins they were.

1. **Size is not the problem.** A mainnet proof is about 160-180 KB on the wire; checking it takes
   tens of milliseconds and a few MB of memory on a desktop. The core library (verify, box proofs,
   key derivation, signing) builds to 2.0 MB for arm64 and 1.3 MB for 32-bit ARM.
2. **Over the peer protocol, nodes serve only a stored proof from the last snapshot height.** 49 of 83
   mainnet peers answered a proof request; 46 sent byte-identical proofs ending at height 1,880,052,
   about 50 days behind the tip (`docs/research/runs/2026-11-13-wide-sync.txt`). The node refuses any other
   parameters or header. So a peer-protocol client gets a proof up to 52,224 blocks old and must
   then fetch every header after it: up to about 11 MB.
3. **Current proofs exist only over REST.** Fifteen peers advertised a REST address; twelve of those
   advertise the same one. Proofs from the three distinct servers all passed the checks and agreed on
   a tip three minutes old (`docs/research/runs/2026-11-13-rest-check.txt`).
4. **The proof format does not bind interlinks to header work**, so the prefix of a proof carries no
   weight in the checks. What is verified is: genesis id, ten recent linked headers with real work at
   a checked difficulty, and the epoch headers that difficulty rests on. That is checkpointed SPV in
   a NiPoPoW envelope. The superchain argument needs the binding proposed upstream.
5. **The published Rust verifier cannot be used as is**: on its development branch it accepts a first
   proof unvalidated and fails real interlink proofs. tachys carries its own checks.

6. **A wallet with a recent checkpoint needs no proof at all.** Peers push their tip header unprompted
   in their sync message (the reference node sends tip, tip-16, tip-128, tip-512; the Rust node sent
   50 consecutive headers). `tachys tips` listened to those, fetched nine parent headers from one
   peer, checked work, floor and linkage, and counted three network groups whose tip sat on that
   chain: 26 KB received, nine requests, tip 197 s old (`docs/research/runs/2026-11-13-tips-1.txt`).
   Caveat from the same run: only 3 of 11 connected peers pushed within 45 s, and the five that did
   not were held open for the full 45 s. The wait must be short, or the push provoked.

7. **Silent peers answer one sync message at once, and the whole tip sync fits in about 16 KB.** Offering
   a verified header that is behind the tip (never ahead: see `docs/good-neighbour.md`) makes a
   reference node reply with its own headers within about 100 ms. Latest run
   (`docs/research/runs/2026-11-13-tips-4-warm.txt`): 5 connections, 3 messages sent, 6.8 connection-seconds
   held with failures counted, 15.7 KB received, 4.2 s, five network groups agreeing on a tip 106 s
   old. The ten-header chain came from headers a peer had already pushed.

8. **Box existence, and absence, can be proven against the agreed tip for a few KB.** `tachys boxes`
   takes box ids handed over by a payer (no index, no address revealed) or looks an address up on
   an indexed node, and checks the node's batch proof against the state root of a header the peers
   agreed on. A made-up id comes back proven absent. Receipt path: one request, 3.3 KB
   (`docs/research/runs/2026-11-13-boxes-1.txt`). The proof endpoint is REST-only today.

9. **A wallet can keep its own state from receipts.** `tachys wallet new` makes a key, an address and
   a birthday height; `wallet receive --ids` accepts a box only if it is proven unspent *and* pays
   our address; `wallet status` re-proves the tracked boxes in one request and drops spent ones;
   `--reconcile` adds an address lookup. Live: a real box paying someone else and a made-up id were
   both refused (`docs/research/runs/2026-11-13-wallet-1.txt`). The accept path has not run live yet: the test
   address holds nothing. Research only: the seed phrase is stored in plain text (sealed since finding 41).

10. **Restore is seed phrase to address to current boxes, with no history.** `tachys wallet restore`
    derives the address, asks each given node what it lists, and proves the union against the agreed
    tip. A funded test wallet with history came back as 21 boxes and 20.29 ERG, the same from two
    nodes and matching an independent explorer's confirmed balance, for 59 KB
    (`docs/research/runs/2026-11-13-restore-1.txt`). A later `wallet status` re-proves the tracked boxes
    without any address lookup. Only the first address of the standard path is derived.

11. **The whole payment loop runs on testnet.** A restored payer wallet built and signed a 0.1 ERG
    payment, a testnet node accepted it, and the payer printed the output box id as the receipt. After
    one block the payee wallet accepted that receipt (proven unspent, pays its address: 1.7 KB, no
    address lookup), and the payer found its change box from its own record, also without a lookup
    (`docs/research/runs/2026-11-13-testnet-payment-loop.txt`). Balances reconcile to the nanoERG including
    the fee.
12. **On a fast chain the node is usually a block ahead of the agreed tip.** The wallet extends its
    chain by up to three headers that link onto the agreed tip and pass the work checks. Those rest
    on one node plus their own work; a box-proof answer that named its header would remove the guess.

13. **Box proofs work over the peer protocol when the node names its block.** A 96-line patch to the
    v6.0.8 node (`node-patches/0001-box-proof-message.patch`) adds a request (ids) and an answer (the
    block id the proof is against, then the proof). Against a patched node on a private devnet
    (`rig/devnet.sh`), tachys proved a real box present and a made-up id absent with no REST call:
    the node was two blocks past the agreed tip, so tachys fetched exactly those two headers by id,
    checked that they link and carry work, and verified the proof against the named block
    (`docs/research/runs/2026-11-13-devnet-boxproof-p2p.txt`): 4 requests, 1.6 KB. Prototype only: no rate
    limit yet, and the node side was exercised by this one client.

14. **Declaring the truthful light mode makes tachys a quieter neighbour.** The reference node consults a
    peer's declared mode in three places. Measured on our devnet: a node bootstrapping from a NiPoPoW
    proof asked a no-mode tachys for a proof 10 times in 30 s, and a light-mode tachys never
    (`docs/research/experiments/2026-11-13-mode-declaration.md`). A normally syncing node asked tachys for
    nothing under any declaration, but sent it 50 KB of inventory in 30 s. tachys now declares the
    light mode by default; no new protocol surface was needed.
15. **Offering an old header is expensive.** A first-run sync that offered the release checkpoint to
    every peer drew about 400 announced header ids from each (66 KB in total). tachys now switches to
    a recent verified header as soon as any peer supplies one.

16. **The wallet flow passes as a rig recipe.** `rig/light-wallet.json` and `rig/light-wallet.sh` run on
    the peeryard rig: a v6.0.8 miner carrying the box-proof patch and an unpatched v6.0.8 release
    follower, each in its own namespace. In one run (`docs/research/runs/2026-11-13-rig-light-wallet.txt`): tip
    sync from both nodes; a payment from the node's wallet accepted by receipt over the peer protocol
    (one request, 366 B, the accept path's first live run); the same request to the unpatched node
    got no answer, no penalty and no ban, and that node kept answering afterwards; tachys paid back
    and found its change from its own record, balance exact.
17. **The fee output's script depends on the network's reward delay.** The Rust library hard-codes
    mainnet's; a network with another delay refuses the payment as carrying no fee. tachys takes the
    script per network.

18. **One wallet can learn the tip from another, with no full node in its path.** `tachys serve` pushes
    the chain a wallet last verified to any peer that connects and answers header requests for it.
    On the rig (`rig/light-overlay.*`, `docs/research/runs/2026-11-13-rig-light-overlay.txt`) a wallet in a
    namespace with no route to the full node synced from a serving wallet: one connection, nothing
    sent beyond the handshake, 2.2 KB received, and the same tip the full node holds. When the
    serving wallet handed out a chain with one header's work broken, the client refused it. The
    client applies the same work, link and floor checks to a wallet as to a node, so a serving
    wallet is just one more voice; what it serves is static until it syncs again.

19. **A wallet with no connection can still pay, through another wallet.** On the rig
    (`rig/store-forward.*`, `docs/research/runs/2026-11-13-rig-store-forward.txt`) a payer wallet cut off from
    the node signed a payment offline and wrote it as a parcel (291 B of transaction); a carrier
    wallet took the parcel over a wallet-to-wallet connection and later submitted it to the node the
    way nodes do among themselves (announce the id, deliver on request); the payment confirmed, and
    the payer, once reconnected, found its change from its own record. No wallet made a REST call
    anywhere in the recipe: tip, box proofs, handoff and submission all went over the peer protocol.
20. **A carrier checks a payment before it submits it.** Once online, it proves every input unspent
    against its agreed chain and verifies the spending proofs against those boxes. In the same recipe
    three bad parcels were refused before anything reached the node: one with a byte of its
    spending proof flipped, one naming an input box that does not exist, and the genuine payment
    handed over a second time after it had confirmed (recognised as already on chain). The node's
    mempool never held the bad ones. Still open: carrying only for wallets one knows.

21. **A payee with no route to any node can check a receipt through another wallet.** `tachys serve
    --relay-proofs` passes a box-proof request to its own node and hands the answer back unchanged,
    together with any newer headers the answer depends on (fetched by id, linked and work-checked
    before they are served). On the rig (`rig/offline-payee.*`,
    `docs/research/runs/2026-11-13-rig-offline-payee.txt`) a payee linked only to the relaying wallet accepted
    a 0.1 ERG receipt (6 requests, 7.3 KB, five newer headers), saw a made-up receipt proven absent,
    and got nothing from a serving wallet with relay off. The relay adds reach, not trust: the payee
    verifies the proof and the headers itself. Passed three runs in a row after the fix below.
22. **The reference node discards anything that arrives in the same read as the handshake.** It parses
    the handshake out of the first chunk and drops the rest, so a client that sends its first message
    straight after its handshake sometimes loses it. Real-network latency hid this; the rig's
    zero-latency links showed it as an intermittent failure. tachys now waits 250 ms after the
    handshake. A cleaner design for the box-proof answer is to carry the block's header itself, not
    its id, which would also save a round trip.

23. **The whole command-line wallet builds for Android and is small.** With the `phone` profile (size
    optimised, same code): 3.37 MB for arm64 and 2.34 MB for 32-bit ARM, including the peer client,
    the vendored codec, an HTTP client with TLS, signing and all wallet commands. On the desktop the
    same build peaks at about 5.5 MB of memory for every operation measured (tip sync, re-proving
    21 boxes, verifying a 162 KB proof), with 0.07 s of CPU for a tip sync
    (`docs/research/runs/2026-11-13-resource-profile-host.txt`).
24. **It runs under Android 7 with 1 GB of memory.** In the Android emulator (API 24, x86_64, 1 GB RAM,
    2 cores) the wallet binary, pushed with adb, synced the mainnet tip (five network groups agreed),
    generated a wallet, and proved a funded address's 21 boxes against the agreed tip. Peak memory
    was about 3 MB in every step; verifying a 162 KB proof took 0.09 s
    (`docs/research/runs/2026-11-13-android-emulator.txt`). This is Android, not ARM: speed on a real low-end
    phone is still unmeasured.

25. **A serving wallet can keep what it serves current.** `tachys serve --follow-state` re-reads the chain
    its own `tachys tips` last verified and serves that. On the rig a client that reaches only the
    serving wallet saw height 21, then height 37 after the serving wallet had synced three more
    times with the node at 38 (`docs/research/runs/2026-11-13-rig-light-overlay.txt`, step 4).

26. **It runs on a real phone.** On a Pixel 6 (arm64, GrapheneOS, Android 17) over Wi-Fi, the arm64 build
    pushed with adb synced the mainnet tip (five network groups agreed, 24 KB first run, 12 KB
    second), generated a wallet, proved a funded address's 21 boxes, and verified a 162 KB proof in
    0.19 s. Peak memory was 7.6 MB (`docs/research/runs/2026-11-13-pixel6.txt`). A Pixel 6 is far above the
    target device; an old 32-bit phone is the run that matters and is still to do.

27. **A first app runs on a phone.** `android/` is a Kotlin and Compose app with two screens modelled on
    the Ergo mobile wallet (wallet list, wallet detail) over the tachys core, which for now is the
    command-line binary run as a child process. On the Pixel 6 it watches a funded address, syncs the
    mainnet tip from peers, proves the address's 21 boxes, and shows the balance and six tokens with
    names from a small in-repo list (screenshots were taken; they are not in the public tree). The APK is 2.69 MB for arm64 and 2.29 MB for
    32-bit ARM, core included; cold start measured 0.8 s and 1.5 s; memory 151 MB PSS on that
    high-resolution phone, of which 46 MB is graphics. Speed and memory on an old phone are still to
    measure, and decide whether Compose stays.

28. **It runs well on an old phone, and Compose stays.** On a Galaxy Note 2 (2012; Exynos 4412 at 1.6 GHz,
    32-bit ARM, 1.8 GB RAM, LineageOS on Android 7.1.2) the command-line wallet synced the mainnet tip
    in 6.6 s with 2.3 MB peak memory, proved 21 boxes with 1.9 MB, and verified a 162 KB proof with
    its 127 work checks in 1.0 s, about eleven times the desktop's time
    (`docs/research/runs/2026-11-13-note2.txt`). The app installed from the 2.29 MB APK, cold-started in 0.7 to
    0.9 s, and showed both screens with the proven balance and tokens (a screenshot was taken; screenshots are not in the public tree);
    Android reported 13 MB for the app (this ROM does not count graphics memory) and a median frame
    time of 8 ms with a 90th percentile of 113 ms over the first 51 frames. The operator's read: it
    works the same as on the Pixel 6.

29. **A payment from one phone to another, made in the app.** On testnet, a wallet created in the app on the
    Pixel 6 was funded from this machine, then paid 0.1 ERG to a wallet created in the app on the
    Galaxy Note 2 using the Send screen. Afterwards the Pixel showed 0.1989 ERG (0.3 less the payment
    and the 0.0011 fee, its change proven and no longer pending) and the Note 2 showed 0.1 ERG, each
    proven against the peers' agreed tip (a screenshot was taken; screenshots are not in the public tree). Both phones found their
    new boxes through the sync's address lookup; the Receive screen's receipt button was not exercised
    on a phone, because driving it by adb kept tripping over the keyboard. Flaws this exposed and
    fixed: autocorrect altering ids, the keyboard covering buttons and pushing the header off screen.
    Still open: the receipt disappears from the Send screen when you leave it; a hardware Enter key
    activates the header.

30. **Scanning and a Pending screen.** The app reads QR codes with the old camera API and the ZXing reader
    (the only library added; about 45 KB after shrinking, APKs now 2.77 MB and 2.38 MB), for a
    recipient's address, an address to watch, or a payer's receipt, which is checked as soon as it is
    read. Pending shows change waiting to confirm, payments waiting for network, and the last ten
    payments made from the phone with their receipts, kept locally and asked of nobody. Installed on
    both phones; the Pending screen and the camera permission prompt were seen on the Pixel 6. A
    scan itself has not been tested yet.

31. **Tokens, purpose, offline payments, and a simpler layout (after the operator's hands-on test).** Scanning
    worked in both directions between the two phones. Changes from that feedback: the QR button sits
    at the end of the field it fills; Send has an optional purpose, like a cheque's memo line, which
    travels in the receipt's QR code and is not written to the chain; Send can pay a token (the core
    builds, signs and tracks token payments, tested on testnet with a token minted for the purpose);
    a token the app has no shipped name for is looked up once on a node and stored, shown as "name
    as reported"; with no network, Send signs the payment and leaves it waiting, and the next sync
    that reaches the network sends it (core path tested on testnet from this machine; not yet tested
    on a phone). Details now reads: address, balance with a Pending line beneath it that opens the
    pending payments, tokens, then History with the finished payments and their receipts. The footer
    is Wallets and Settings.

32. **The payee can hold the payment itself.** When a payment is signed with no network, the receipt's QR
    code carries the signed transaction (334 bytes for a simple payment), not just a box id. The
    payee's wallet keeps it, expects the boxes that pay it, submits it on its own next sync, and adds
    the box only once proven. Tested on testnet from this machine: the payer signed and never sent;
    the payee's sync submitted it and proved 0.03 ERG. The app words this as a promise, like a
    cheque, until it is in a block. A payer who never sends is defeated; a payer who spends the same
    coins elsewhere first is not, and cannot be without funds committed in advance
    (`docs/design.md`). Not yet tested between two phones.

33. **Several payees, consolidation, burning.** One payment can pay several addresses for one fee
    (`wallet send --pay ADDRESS:NANOERG`, repeatable); `wallet consolidate` merges a wallet's boxes
    into one, which also shrinks later proofs; `wallet burn` destroys part of a token. Each ran on
    testnet: three payees in one transaction, 5.00 of a test token burned, three boxes merged into one.
34. **A wallet for programs and agents.** `tachys balance`, `tachys pay` and `tachys accept` each print
    one JSON object and need no knowledge of boxes or fees (`docs/agents.md`). `pay` obeys a policy
    file the wallet enforces itself: a cap per payment, a cap per day, an optional list of payees,
    tokens off unless allowed; with no policy file, paying is off. Every payment goes to a log with
    its purpose. On testnet: a payment within limits went through and its receipt was accepted by
    the payee; a payment over the limit, a token payment, and a payment with no policy were refused
    (`docs/research/runs/2026-11-13-agent-cli.txt`). The seed lists now ship inside the binary.

35. **Signing requests.** The wallet takes a static ErgoPay request (`ergopay:` followed by a reduced
    transaction, the form FreeDEX's phone route produces), says what the transaction does in its own
    words from the transaction itself (what you give, what you get back, what others put in and
    receive, the fee, and the net), proves any input that is not the wallet's own, and signs and sends
    only when told. On testnet from this machine a request was read, signed, sent and confirmed; a
    request that spends nothing of the wallet's, and one that holds no transaction, were refused
    (`docs/research/runs/2026-11-13-signing-request.txt`). In the app, "Sign a request" scans one, and an
    `ergopay:` link opens the app on the same screen; the Pixel 6 opened a request this way and
    displayed the reading. **A flaw this caught:** the first reading counted a box at the wallet's own
    address as someone else's money because the phone was not tracking it yet, and understated the
    cost tenfold; ownership is now decided by who can spend the box. On the Galaxy Note 2 the whole
    flow then ran in the app: the link opened it, the reading was right (two inputs, both the
    wallet's; net -0.0111 ERG), "Sign and send" was pressed, and the chain showed the two inputs
    spent and the change box created (a screenshot was taken; screenshots are not in the public tree). Not yet tested: a request
    built by FreeDEX itself (it has no testnet mode, so that means mainnet), and requests involving
    tokens or a contract box.

36. **A real swap, built by FreeDEX, read and signed by tachys, on mainnet.** The live FreeDEX page's
    phone-signing route built a swap of 0.2 ERG for SigUSD against the ErgoDEX pool. The wallet's own
    reading: two inputs, one the wallet's and one the pool box (proven unspent, 149,612 ERG deep);
    net -0.201 ERG and +0.03 SigUSD, the page's quote exactly. The Pixel 6 showed the same reading on
    its read-only wallet. Signed and sent from the machine holding the key; the node accepted it
    under the transaction id the page had shown, and after one block the wallet proved 20.092868012
    ERG and 0.03 SigUSD (`docs/research/runs/2026-11-13-mainnet-freedex-swap.txt`). This is the first
    contract input and the first two-way token movement through the wallet. A request replayed
    after it confirmed is refused (its input is spent). The reading now names tokens it knows and
    leaves out tokens that pass straight through.

37. **Wallet management, after the Ergo mobile wallet.** Several wallets per network, each in its own
    folder, sharing one agreed chain tip (fetched once per sync, not once per wallet). The "+" on the
    wallet list offers New wallet (shows the seed phrase to write down), Restore wallet and Read-only
    wallet. Restoring the phrase of a read-only wallet already on the phone turns it into a full
    wallet and keeps what it tracks; a different phrase over a read-only wallet is refused (both run
    on testnet in the core). Each wallet's gear page has its addresses, its name, Add an address,
    Display public key, Display mnemonic and Remove wallet. The phrase and removal ask for the
    phone's own screen lock; while a phrase shows, the screen cannot be captured. Settings gained an
    app lock (the phone's screen lock, asked whenever the app is reopened) and the node address per
    network. **Several addresses:** keys at m/44'/429'/0'/0/i; the balance counts all of them and
    change returns to the first. On testnet a wallet funded at two addresses made one payment that
    had to spend from both (`docs/research/runs/2026-11-13-two-addresses.txt`). **Public key:** laid out as
    ergo-appkit's Bip32Serialization does; the core reads the key back out of the bytes and derives
    the wallet's first address before showing it. Not checked against the Ergo mobile wallet's own
    display. **Not yet run on a phone screen:** the new app screens are built and the app starts on
    the Pixel 6 with its wallets, but nobody has tapped through them.

38. **Fees in a token (EIP-31, Babel fees).** `wallet send --babel TOKEN` pays the ERG a payment lacks
    out of someone's standing offer, putting tokens back into it at the offer's price. Offers are
    listed by a node, then each is proven unspent under the agreed tip and checked to sit under the
    Babel contract for that token: a node can hide an offer, not invent one. The cheapest offer that
    covers the shortfall is used. `wallet babel-offer` creates an offer and `wallet babel-list` shows
    the proven ones. On testnet a wallet with 500 token units and 0.0029 ERG could not make a payment
    needing 0.0036 ERG (refused: not enough coins); with the fee in tokens the same payment was
    accepted and confirmed, taking 0.0017 ERG from an offer for 17 units, and the offer box came back
    with that much less ERG (`docs/research/runs/2026-11-13-babel-fees.txt`). The app's Send screen has the
    choice for token payments. Not done: withdrawing one's own offer, showing the price before
    sending, chaining on an offer already spent in the mempool, and any run on mainnet or on a phone
    screen.

39. **What became of a payment.** The wallet remembers each payment it sends: the boxes it spends, one
    box it creates, and the signed transaction. Each sync reads the outcome off the same proof as the
    balance, asking nobody: its box exists, so it is confirmed; a box it spends is gone and its own
    box is absent, so its coins were spent elsewhere and it can never confirm; neither for ten
    blocks, so it is not included and its coins count as the wallet's again; otherwise it is waiting,
    and is sent again in case a node dropped it. While it waits, the coins it spends are not counted
    or spent again. A payment given up on is still watched, and becomes confirmed if it is included
    late. Confirmations are a lower bound: the tip height less the height at which the wallet first
    proved the payment in the chain, plus one. All four outcomes ran on testnet
    (`docs/research/runs/2026-11-13-payment-tracking.txt`). The app shows them under Pending and History, and
    the Send screen now looks up the best Babel offer and shows the price before anything is sent.
    Built for the phones and run on the Pixel 6 (finding 40).
40. **The new screens, by touch, on the Pixel 6 (testnet).** Worked as built: the plus page with its
    three choices; restoring a wallet from its phrase; the gear page (address, adding a second
    address, renaming, the extended public key); the seed phrase stopping at the phone's own
    screen-lock prompt, and nothing shown when the prompt is dismissed; Receive stepping between
    addresses; Sign opening the scanner with a wallet chooser; a token payment whose fee was paid in
    the token (1 TACH sent, 0.21 TACH taken for the fee, from a wallet holding 0.001 ERG), shown as
    waiting, then confirmed with a count after one block. Faults the screen found, all fixed:
    the offer lookup failed whenever the node was a block ahead of the wallet's agreed tip (it now
    refreshes the tip once and retries, and the core says why instead of "no offers"); a lookup
    could collide with the start-up sync (tip refreshes now run one at a time); the fee-in-token
    choice was offered to a wallet with enough ERG, which the core then refused (it is now offered
    only when ERG is short); the answer to Send appeared below the edge of the screen (the screen
    now scrolls to it); a page opened at the previous page's scroll position. Not exercised: app
    lock and seeing the phrase (both need the owner's screen lock), changing the node, a scan
    through the camera, mainnet, and the Note 2.

41. **Seed phrases are sealed at rest, the way ergo-wallet seals its secret.** Given a password
    (`--pass-stdin`, or `TACHYS_PASS`), `wallet new`, `wallet restore --embed` and `wallet encrypt`
    (which converts a plain-text wallet) write the phrase as ergo-wallet's `EncryptedSecret`: a key
    from the password by PBKDF2-HMAC-SHA256 (128000 rounds, 256 bits, 32-byte salt), then AES-256-GCM
    with a 96-bit nonce, in the same JSON fields. Checked both ways against a Java program that
    repeats ergo-wallet's `AES.scala` line for line: it opens what tachys sealed and tachys opens
    what it sealed (`docs/research/runs/2026-11-14-seed-encryption.txt`). Two things read off the source, not
    the docs: `AES.scala` cuts the cipher output after 16 bytes, so the field called `authTag` holds
    the first 16 bytes of ciphertext and `cipherText` ends with the real tag (kept as is, for
    compatibility); and ergo-wallet seals the 64-byte seed, where tachys seals the phrase, because a
    seed cannot be shown back as words. A sealed wallet without its password still knows its
    addresses, so it syncs, proves and receives; it refuses to sign, add an address, or show the
    phrase or public key. A wrong password fails the tag check; a sealed phrase that does not lead
    to the wallet's address is refused. Without a password the CLI keeps the phrase in plain text as
    before, and says so. **On the phone** the password is 32 random bytes that no one sees, wrapped
    by an AES key in the Android keystore that works only for 15 seconds after the phone's screen
    lock is given. Send, Sign and Display mnemonic always ask for the screen lock; creating or
    restoring a wallet, adding an address and the public key ask unless it was just given. The
    password and a phrase being restored reach the core on its input, never as an argument or a
    file. Wallets from earlier builds are sealed on the first unlock. Two limits: removing the
    phone's screen lock makes Android destroy the key, after which the phrases on the phone cannot
    be opened and each wallet is put back from its written phrase (Restore does this in place); and
    a phone with no screen lock gets encryption at rest but no guard, until a lock is set.

42. **Send, after the Ergo mobile wallet: ERG and several tokens in one payment.** `wallet send`
    takes further tokens as `--also-token ID:RAW` (repeatable) beside `--token`; all go to the payee
    in one box. Built unsigned from a copy of a mainnet wallet holding seven tokens and read back by
    the wallet's own request reader: 0.005 ERG and three tokens gave exactly those to the payee; all
    ERG less the fee with every token in full left no change box; the same with tokens kept is
    refused unless 0.001 ERG stays for the change box. The app's Send screen has an ERG row and a
    row per chosen token, each with what is available and a Max button (Max for ERG leaves the fee,
    and 0.001 ERG if any token stays behind); tokens are added from a list that opens in place. A
    scanned payment request (`…/payment-request?address=…&amount=…&description=…`, or `ergo:`) fills
    the recipient, the ERG amount and the purpose; the format was checked against the Ergo mobile
    wallet's parser. Scan sits in the bottom bar: scan first, then choose the paying wallet; a
    signing request goes to the Sign screen, so the wallet card no longer has a Sign button. A name a
    node reported is marked with an asterisk and explained once beneath the list. Seen on the Note 2:
    the screen, the token list, and Max. A payment of 0.01 ERG with two tokens (643 CHEPUR and 100
    SigRSV) was then sent from the Note 2 on mainnet: tx
    `0ddf5fb9…`, one payee box holding both
    tokens, in block 1917020 (public explorer API).

43. **The tip is chosen by work, not by height.** Before: `tachys tips` took the highest tip any peer
    pushed, and a header's height is whatever its maker wrote; ten headers at the release floor (a
    fifth of mainnet's real difficulty on 2026-11-14: floor 1.47e13, real 7.3e13) from enough network
    groups would have been agreed on while honest peers pushed the real tip. Worse, read off the same
    code: a header the REST node appended to the agreed chain needed only to link and to clear that
    floor, so one block at a fifth of the difficulty from one source could carry a false state root.
    Now (`crates/tachys-verify/src/tip.rs`): (1) pushed tips are tried heaviest first, up to three
    chains, and a chain is refused while a recent tip claiming more difficulty has been heard that is
    not on it; a heavier tip that is on the chain (an ancestor from before difficulty fell) or older
    than the tip-age limit is not a rival. (2) The floor remembers: the newest header of the last
    agreed chain (`agreed-tip.bin`) sets the least difficulty a header may claim, halved once at once
    and once more each time the elapsed time doubles past an epoch (1 + floor(log2(t / 4h16m + 1))
    halvings), which is looser than EIP-37's own limit allows the honest chain to fall. On mainnet a
    second sync raised the floor from 1.47e13 to 3.66e13, half the real difficulty. (3) A header
    added on top of the agreed chain must be the next height at the same difficulty, or at the first
    block of an epoch no less than half. (4) Chain files are written only when the verdict is Agreed
    (they used to be written before the verdict). Ten unit tests on real mainnet headers and altered
    copies (a forged chain with a quorum is refused once one honest tip is heard; with an honest
    quorum the real tip wins over a higher forged one). Live: mainnet and testnet syncs agree as
    before; a mainnet balance was proven after the node moved one block past the agreed tip
    (`docs/research/runs/2026-11-14-tip-by-work.txt`). What it does not do: with every connection in an
    attacker's hands and nothing remembered, the cost of a false tip is still ten headers at the
    release floor; with a tip remembered from the last day it is ten headers at an eighth to a half
    of real difficulty. Testnet's release floor is zero, so testnet proves nothing about forgery.

44. **Three outside reviews, checked against the code, and what changed (2026-11-15).** The tree was
    given, with no other context, to a fresh Claude session, to Antigravity and to Grok, with one
    question: how could someone using this wallet lose funds, leak a key, be shown a balance or a
    payment that is not real, or sign something other than what the screen showed. Every finding was
    checked by reading the code or running it before anything was changed. What held, and what was
    done: (a) **A fee paid in a token had no ceiling.** The core took the cheapest offer the node
    listed, up to the wallet's whole balance of that token, while the app's "at most" came from a
    separate lookup; a node that left the good offers out of the second list set the price. `--babel`
    now needs `--babel-max RAW` and nothing is signed above it; the app passes the figure it showed,
    and quotes only offers large enough to be used. (b) **A signing request counted only boxes at the
    wallet's address as "yours".** A box under a contract that the wallet's key can open (an offer it
    made) was listed under "others put in", so the line "net for you" could show dust while the
    signature emptied the offer. An input is now the wallet's when its spending condition, as the
    request states it, names one of the wallet's keys; the wallet's own record of a box is used only
    if its bytes hash to its id. (c) **One header from one source could become the tip.** A peer
    whose tip was an ancestor counted toward the quorum for whatever sat above it, so one peer
    pushing one block of its own, at the chain's difficulty, had the others' agreement; and the node
    asked for a proof could add three headers (a peer, twelve). Now the agreed tip is the newest
    header that a quorum of network groups have reached. Headers above it are kept for reading
    proofs and nothing more: a box proven only there is "seen", with no confirmations, and `accept`
    and the Receive screen wait for the peers to reach it. Incoming payments show their
    confirmations (a lower bound, counted from the first proof under an agreed tip), and a proof is
    read only against the newest four stored headers, so a balance is at most a few blocks old.
    What is taken away follows the same rule (found by a second pass of the first reviewer): a box is
    dropped, and a sent payment called confirmed or failed, only on a proof at or below the agreed
    tip.
    (d) **A heavier header from before a difficulty drop, replayed by one peer, blocked agreement for
    up to three hours.** The wallet now asks for the chain's ancestors down to that header's height
    (up to 128, each the parent of the last) and counts it as explained if it is among them. (e) **The
    spending policy guarded only `pay`**, counted a token payment as 0.001 ERG whatever its amount,
    left the fee out, read a missing log as nothing spent, and let two payments at once read the
    same total. With a policy file, every command that signs is now held to it
    (`crates/tachys-cli/src/policy.rs`, `docs/agents.md`): the limits apply to what the transaction
    takes out of the wallet, fee included; a token needs limits of its own; the log must exist, and
    the payment is written to it under a lock before it is signed; `wallet mnemonic` is refused. It
    binds only a caller that cannot read the wallet's directory: a program running as the wallet's
    own user can copy the wallet to a directory with no policy, so an agent has to run as another
    user and reach the wallet through a wrapper that allows `balance`, `pay` and `accept` alone.
    (f) **Any HTTP error from the node deleted a held payment**, which for an offline payment is the
    payee's only copy. A refused payment is now kept until a proof shows that it is in a block or
    that a box it spends is gone; after twenty refusals it is moved out of the queue into
    `given-up/`, not deleted. A payment carried over the peer protocol is dropped only on a refusal
    that the chain or its own signatures settle, not on a failed connection. (g) **The wallet file was
    readable by other users of the machine.** It is written for its owner alone, and so is a state
    directory the wallet creates. (h) **The release build** carried the build machine's home
    directory about 320 times in the core (the source paths in panic messages) and kept the adb test
    hooks in an activity any app can start. `android/build-core.sh` remaps the paths and fails if one
    is left; the hooks are in debug builds only. (i) Smaller: an id that was not hex could stop the
    core (it was sliced by bytes); success was read off the word "accepted" anywhere in the output,
    including a node's refusal; requests to a node had no read timeout (now 10 s to connect, 30 s to
    read); restore looked only at the first address (it now looks at twenty and keeps those up to
    the last that holds coins) and took a mistyped phrase as the key to an empty wallet (the word
    list and checksum are now checked; `--any-phrase` skips it); the phrase being typed on the
    restore screen could be captured; a token name a node reported was unmarked on the Sign screen
    (it now carries the asterisk and the start of its id); two quick taps could both start a send.
    **What the reviews got wrong:** `pack_interlinks` was said to mishandle repeated ids, and it
    matches the node's own `packInterlinks` (`NipopowAlgos.scala`), which counts over the whole
    vector; the extended public key was said to need a hardened child number, and its four zero
    bytes are what ergo-appkit writes; several quoted snippets are not in the tree; and a proof
    against an older stored header was rated as a way to pass off a spent receipt, which it is not,
    because a box that pays this wallet can be spent only by this wallet (it did allow a balance
    several blocks old). **Tests:** 12 on the tip rule and 9 in the core (the policy, the ownership
    rule, the phrase check, text from a node); on testnet, with a copy of a wallet and nothing
    broadcast: the policy's limits, the fee ceiling, file modes, a mistyped phrase, a node sending
    ids that are not hex, and a held payment against a node answering 500 and 400
    (`docs/research/runs/2026-11-15-review-fixes.txt`). On a phone (a Galaxy S5 on Android 11, 32-bit, testnet,
    driven over adb): a sync with the new tip rule; a receipt accepted with its block and "at least 3"
    confirmations; the restore screen refusing a screenshot; and the release build started with the
    test extras staying on mainnet with no wallet planted. **Not run:** the ownership rule against a
    real offer box; a held payment whose coins are gone; a peer pushing a block of its own against a
    live client; and, on a phone, a fee paid in a token, the Sign screen, and a mistyped phrase on
    the restore screen (it stops at the screen lock, which adb cannot give). **Not fixed:** the decimal
    places of a token not shipped with the app are still whatever a node reported; a malformed box
    proof can still stop the core (the proof verifier slices unchecked, and the phone build aborts
    on a panic); a held payment the node accepts and later drops is gone from the payee's phone;
    and `cargo audit` reports RUSTSEC-2026-0387 (timing in `crypto-bigint` 0.5.5), which comes with
    sigma-rust's `k256` and cannot be fixed from here.

45. **A fourth review, and two more read against the code (2026-11-15).** **Changed.** (1) A wallet
    file that is there but cannot be read (cut short, damaged) used to count as no wallet, so
    `wallet new` and `wallet restore` wrote a new one over it, seed phrase and all; it is now refused
    and left untouched. (2) `tachys tips` exits 1 when no tip is agreed on (it exited 0), and says
    Agreed only after the chain is on disk; chain files, a promised payment and the token names are
    written whole or not at all. The agent commands and the app read the exit code as well as the
    line. (3) An option's value that cannot be read (`--peers lots`), an option that does not exist
    (`--nanoergs`) and a `wallet` command that does not exist are refused; each used to be read as
    "not given" and run with a default. (4) A seed list is used only when named with `--seeds`; a
    `seeds-mainnet.txt` that happened to sit in the current directory used to replace the shipped
    list. (5) A proof is read only against a stored chain whose newest header is under three hours
    old, the limit a tip is already held to; `wallet receive` on a state directory left for days
    used to accept one. (6) Finding 39's rule called a payment failed when a box it spends was gone
    and the one box it watched was absent. A payee who had already spent that box made a confirmed
    payment read as failed, which invites paying twice. Every box the payment creates is now asked
    about; "failed" is said only when the payment pays change back to this wallet, that change is
    absent, and this wallet has not spent it; otherwise the answer is that the coins are spent and
    the proof cannot say by what. (7) Network groups: an IPv6 address counted as a group of its own,
    so one machine with a /64 could be a quorum; an IPv6 /32 is now one group. (8) At the chain's
    largest fall in difficulty, exactly half, the header's compact form rounds down, and a header
    added by one source was refused for claiming a hair under half; the rounding is allowed for.
    (9) The two test hooks left in release builds (`--tamper-for-control`, `TACHYS_NO_RESEND`) work
    on a private network only. (10) The three copies of the proof check are one, which catches a
    panic from a malformed proof where the build unwinds (the phone build still aborts).
    `tachys sync` and `check` print `proofs verdict:`, so their line cannot pass for the wallet's.
    (11) `cargo test --workspace` failed unoptimised (a vendored test overflowed its stack) though
    2e31413 said it passed; it passes now, and clippy is clean on the tachys crates. **Wrong in the
    reviews:** the sealed phrase's `authTag` and `cipherText` were said to be swapped; they are cut
    as ergo-wallet cuts them (finding 41), and changing it would break every sealed wallet. An upper
    limit on a rise in difficulty was asked for; a header that claims more work has had to do it.
    **Tests:** 21 in the core, among them a batch proof built locally and read back, with every byte
    of it altered in turn. **Run:** each refusal above by hand against a debug build, and one live
    `tachys tips` on mainnet. **Not run:** a real node's box proof through the changed code (no REST
    node answered from the build machine), the rig, and the app on a phone (it compiles). **Not
    done:** one structured (JSON) answer per command in place of printed lines; the core as a library
    with a binding; a versioned wallet file; `tachys sync` on the tip rule.
    **Nodes are found, not fixed.** The one default node (`<own-node>:9053`) did not answer on the day of
    this review, and nothing could be proven or sent. `--node auto`, now the default for the agent commands
    and the app, asks a few well-known nodes (kushti's, Cornell's) for themselves and for the REST addresses
    their peers advertise, and keeps a node only if `/info` and `/blockchain/indexedHeight` show this
    network, the UTXO set, a box index and the chain's height. An advertised address is a peer's word: it is
    taken only as `http(s)://host[:port]` and only if it resolves outside private and loopback ranges.
    Well-known nodes are used first; an advertised one only when fewer than two of those answer, because
    every node on the list is asked what the wallet's addresses hold. The list is kept in
    `<state>/nodes.json` for ten minutes. Proofs and sending move on to the next node when one does not
    answer; a node's refusal of a payment is final. Run on mainnet: six usable nodes found in under five
    seconds, a block's boxes proven through the list, and a dead node first on a list passed over.

46. **The work list from the reviews: structured answers, a typed wallet file, tests for the peer client
    (2026-11-15).** (1) **Every command takes `--json`** (`docs/json.md`): one object on stdout, the sentences
    on stderr, the same exit code. The agent commands and the app now decide on fields and exit codes: a tip
    is agreed when `verdict` is `agreed`; a receipt is accepted when its entry in `checked` is `proven` with a
    confirmation; a payment is sent when `broadcast.accepted` is there. Finding 44 fixed one case of success
    read off a word a node chose; this removes the reading. A test stands a node on loopback that refuses
    with `"broadcast via x: accepted, y\nverdict: Agreed"` in its answer: the payment is `ok: false`, exit 1,
    and not remembered as sent. `wallet send --dry-run` builds the transaction, asks the policy and stops
    before signing; `pay --dry-run` reports that, where it used to report the least a box may hold for a
    token payment. The app still reads the receipt id, the change box and the fee paid in tokens from the
    core's own sentences for showing; they decide nothing. (2) **The wallet file is typed and versioned**
    (`walletfile.rs`, `"version": 1`). The code no longer reaches into untyped JSON; a file from a newer
    build or with a field of the wrong type is refused with the reason and left alone, by the core and by
    the app, which now says so on its first screen where it used to show no wallet. Nine wallet files from
    earlier runs were read and rewritten on copies with nothing lost. (3) **`tachys-net` has tests**: eight
    on loopback, against its own `serve` (headers pushed and fetched back by id, a parcel acknowledged, a
    box proof relayed with the header it rests on) and against a bare listener that misbehaves (no
    handshake, an oversized one, another network's frames, a sync message that does not parse, a box proof
    cut short, a header nobody asked for, an acknowledgement of the wrong length, silence). **Tests:** 900
    in the workspace. **Run:** the agent commands on mainnet through the JSON answers. **Not run:** the app on
    a phone (it compiles), a real payment, the rig.

47. **After finding 46: what a program needs, a payment nobody answered, and more than one wallet on a
    phrase (2026-11-15).** (1) **A broadcast no node answered is not "not sent".** The wallet used to
    record nothing and report a failure when a node took the request and the answer never came; the
    payment could then confirm, or be paid twice. It is now kept as waiting with its coins set aside,
    reported as `outcome_unknown`, sent again by the next sync and given up after ten blocks like any
    other. Only a node that was never reached, or one that refused before any node could hold it, is
    "not sent". (2) **This wallet does not assume it is the only one holding its keys.** Finding 45 said
    "failed" when a payment's change was absent; a second wallet on the same phrase (Nautilus, for
    what this one cannot sign) can spend that change. "Failed" is no longer said from a proof: the
    outcome is "coins spent", with what a node reports of the transaction given as its word. A box gone
    that no payment of ours spent, or such an outcome, marks the wallet so that the next sync asks the
    nodes again what its addresses hold. (3) **No command ends the process.** Sixty-four exits became
    returned errors; `main` alone exits. (4) **The list of options is gathered from the source** by the
    build, not kept by hand. (5) **Recipes for agents** (`docs/agents.md`): `tachys recipes` names, for
    each thing a program wants, the one command to run; `setup`, `address`, `wait` (one call until a
    receipt is accepted or a payment settled) and `history` are new. (6) **The app reads no sentence of
    the core's**: every screen works from fields. Token counts are worded for none, one and many.
    **Run:** the four rig recipes, each PASS (`docs/research/runs/2026-11-15-rig-after-review.txt`); the phone
    build of the core on the Note 2 (Android 7.1.2) for `recipes`, `setup`, `tips --json`, `nodes`,
    `balance`: one JSON line on stdout, the linker warning on stderr, about 16 ms to start a command.
    The app was installed on the Note 2 over the one there and not driven. **Not run:** any screen of
    the app; a real payment; a broadcast lost in earnest. **In-process binding:** not taken up. Starting
    the core costs 16 ms on the oldest phone here against seconds of network, the child process is what
    contains a malformed proof (the phone build aborts on a panic), and the network, the JSON answer
    and the peer magic are still one per process.

48. **Proof that a payment confirmed; the wallet as a library; the last of the old ranking (2026-11-15).**
    (1) **A payment whose boxes are all gone can still be proven confirmed.** Finding 47 left such a payment
    as "coins spent", with a node's word for the rest. Now the block a node names is found on the stored
    chain, or reached from it by following parent ids back (each header must hash to the id the one above
    names, up to 5000 blocks), and the node's Merkle proof must lead from the transaction's id to that
    header's transactions root. A header reached that way is an ancestor of an agreed header, so no new trust
    is taken. `tachys included --tx` does it on its own; a sync does it for a payment it cannot otherwise
    settle, and once for any payment an earlier build left as "coins spent" or "spent elsewhere". No proof can
    show a transaction is absent, so "coins spent" remains for the rest. Run on testnet: the payment of
    finding 47's two-wallet run, 36 blocks back, proven in 4 requests and 31 KB, and its entry put right.
    (2) **The wallet is a library with one way in**, `tachys::call(args, input)`: the network, the answer and
    the peer protocol's magic are set by each call and nothing is left for the next; a password and a phrase
    are handed over as lines; nothing is written to the process's stdout. The agent commands call the other
    commands in the same process, where they used to run the program again. Calls are taken one at a time.
    This is what an in-process binding needs; none is made yet, because the phone build still aborts on a
    panic and a child process is what contains that. (3) **`tachys sync` and `check` judge proofs by the tip
    rule** (work, then a quorum of groups). A test holds the case the old ranking lost: four groups on a
    higher, lighter chain against three on the real one. (4) `tachys tips` on testnet agrees with no options
    (its defaults there are two peers and a quorum of two). (5) The password, the phrase and the key made
    from the password are wiped from memory when done with; what the signing library makes of them is its own
    to wipe. (6) **RUSTSEC-2026-0387 on the phone's processors.** The scalar operations that touch a secret in
    signing and key derivation (add, subtract, multiply, parse) were built for arm64 and 32-bit ARM as the
    phone build is and disassembled: every conditional branch in them and in what they call is a loop over a
    fixed count or inside memcpy; none depends on a value. Point multiplication is k256's own field code and
    was not looked at. (7) A receipt this wallet was paid and has since spent now gets an answer ("spent")
    where it got none. (8) The app: a Boxes page with consolidation, a Copy button on Receive, the logo as its
    icon (`android/make-icons.py`), and a payment line that no longer squeezes its neighbour into a tall gap.
    **Tests:** 908 in the workspace. **Run:** the four rig recipes again, each PASS; live payments on testnet
    and mainnet (`docs/research/runs/2026-11-15-live-payments.txt`); on the Note 2, Receive and History looked
    at and the Copy button pressed. **Not run:** Send, Sign, Restore and Boxes on a phone.

49. **The app's build brought up to date, and tried on Android 7 and Android 17 (2026-11-15).** Android Gradle
    plugin 8.5.2 to 9.5.1, Gradle 8.9 to 9.9.1, Kotlin 2.0.20 to 2.4.21 (it now comes with the Android plugin),
    Compose BOM 2024.09 to 2026.09, compiled against and targeting API 37. Two calls the newer libraries
    deprecate were replaced: the keyboard's autocorrect switch and the clipboard. `-PtachysIdSuffix=.test` builds
    a copy that installs beside the real app with its own wallets. **Run**, as such a copy holding a read-only
    wallet, on a Galaxy Note 2 (Android 7.1.2) and a Pixel 6 (Android 17, where the new target's behaviour
    applies), driven over adb: the forms take typed text; the wallet syncs (22 boxes proven), so the core
    still starts as a child process under the new target; Receive's Copy puts the address on the clipboard
    (read back on Android 7, shown by the system's own overlay on Android 17); the Boxes page lists the boxes;
    the back button steps Boxes, Details, wallets; the screens sit correctly under the status and navigation
    bars. **Not run:** anything that needs the screen lock (create, restore, send, sign, consolidate), which adb
    cannot give. One thing seen: text injected by adb in one long burst lost characters in a field, and in short
    bursts did not; a person typing or pasting was not tried.

50. **Six seats before publication, and what they found (2026-11-15).** The tree was read by six reviewers, none
    of them a person: three with the project's context and one question each (does it work, read blind and by
    running it; is what the documents say true; what will keeping it up take), two other vendors' models and one
    more of this one's with no context at all, each given only the public tree. All said: publish, after fixes.
    **Changed.** (1) **The tip rule let one heavy header stand on nine cheap ones.** `judge` allowed any change of
    difficulty at a height that claimed to open an epoch, and never checked that linked headers claimed
    consecutive heights; ranked by its tip's difficulty, such a chain looked as heavy as the real one and cost
    two or three blocks of work, not ten. Heights must now be consecutive and a change of difficulty within the
    chain's own limits (half to half again). Shown by a test that fails without the change; no forged chain was
    run against a live client. (2) **Only confirmed boxes are the balance, and only they are spent.** A box
    proven only in a block one node had shown was counted, reported under `proven: true`, and spendable. It is
    now tracked and shown apart. A box confirmed on an earlier sync is no longer relabelled "seen" by a later
    proof above the agreed tip. (3) **`accept` said `ok: true` and exited 1**, never said what the receipt's box
    held, and accepted the same box twice. `ok` now follows the exit code on every agent command, and the
    answer carries the box's amount, its tokens and `first_seen`. (4) **`balance` could not see a plain payment**:
    it proved the boxes the wallet already knew and never asked what its addresses hold, so a deposit made
    without a receipt showed as nothing, under `proven: true`; with a dead node it said the same. It asks now,
    and fails if no node answers. (5) **"Not included: you may pay again" could pay twice**: the payment given up
    on is still valid. The wallet's next payment now always spends one of its coins, so only one of the two can
    confirm; and a payment given up on whose coins then go is settled by a proof that it is in the chain. (6) A
    payment is written down as waiting before any node is sent it, and put back on a definite refusal: a stop in
    between used to leave its coins looking free. (7) A payee's copy of an offline payment was deleted when a
    node took it; it is kept until a proof shows it in a block. (8) A mistyped `--network` meant mainnet; it is
    refused. (9) Agent answers carry `v` and, when refused, a `code` to branch on; a slip caught before the
    command runs still answers in JSON. (10) The documents said more than the code in a score of places, most of
    them about what an agent may rely on (a policy "it cannot change", "every box proven", "you may pay again",
    jitter and backoff that were never built); each is corrected. (11) For whoever keeps this up: `SECURITY.md`, an
    issue template that warns against pasting a wallet file, `docs/maintaining.md`, `tachys version` with the
    checkpoint's age (also in every `tips` answer, with a warning past sixty days), the accepted advisory written
    down, a pinned toolchain and a weekly run in CI. (12) The project's own node address, wallet addresses and the
    screenshots are out of the tree, and box and transaction ids in the logs are cut short. **Wrong in the
    reviews:** that `File::lock` does not exist and the tree does not compile (it is in the standard library from
    1.89, now declared); a run log that is not there; the node address "on line 1" of the seed lists. **Not
    changed, and said in the roadmap:** a node always a block ahead settles nothing until a sync lands on an
    agreed block; testnet has no floor to speak of; a restored wallet empty at all twenty addresses keeps only
    the first; nodes are asked over plain HTTP; a found node's name is resolved again on each request.
    **Tests:** 912 in the workspace. **Run:** after the changes, `balance`, `accept` and `tips` on mainnet and
    testnet; the four rig recipes, each PASS; and on testnet a payment through the changed path: paid, the payee's
    `accept` refused with `not_yet`, its `wait` returned the receipt's amount with `first_seen`, and the payer's
    `wait --tx --confirmations 2` returned at two. **Not run:** a forged chain against a live client; the app on
    a phone since these changes (it compiles).

