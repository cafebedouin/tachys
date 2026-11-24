//! `wallet status`, `receive`, and the sync after a restore: the wallet's boxes are proven against the agreed chain, and
//! what became of each payment it sent, or holds for itself, is settled from those proofs.

use super::*;

/// One proof, and what it was asked: the boxes asked about and, for each, its bytes if it is unspent at `height`.
/// `settled`: the proof is at or below the tip the peers agreed on (`agreed`), so it can take away as well as add.
#[derive(Clone, Copy)]
pub(super) struct Shown<'a> {
    ids: &'a [String],
    found: &'a [Option<Vec<u8>>],
    height: u32,
    agreed: u32,
    settled: bool,
    chain: &'a [Header],
}

/// What a sync changes as it reads a proof: the boxes expected, the boxes that can never be, and the boxes undone.
pub(super) struct Books {
    expect: Vec<String>,
    void: Vec<String>,
    undone: Vec<String>,
}

/// What settling the wallet's own payments leaves for the boxes to be read by.
pub(super) struct Sent {
    /// boxes that payments of this wallet, waiting or settled, spend
    spent_by_us: Vec<String>,
    /// boxes a payment still waiting spends: not this wallet's to count or spend again
    held: Vec<String>,
    /// boxes a payment of this wallet spent or made: nothing to say of them on a sync
    quiet: Vec<String>,
    /// a payment's outcome was unclear: the nodes are to be asked again what the addresses hold
    refresh: bool,
}

/// `wallet receive --pools`: a box asked about that this wallet neither has nor expects may be paid by a payment not yet in a block.
/// If a node holds that payment in its pool it is fetched (`pending_payment`: it is the payment that makes that box,
/// as far as its body goes; a payment's id does not cover its signatures). It is held as if the payer had handed it
/// over (`hold`) only if every coin it spends is proven unspent against the agreed chain and its spending proofs
/// verify against them: then it is a payment that can be made, and from then on its coins are proven with the box.
pub(super) fn take_from_pools(
    c: &Ctx,
    w: &mut WalletFile,
    my_trees: &[ErgoTree],
    nodes: &[String],
    ids: &[String],
    cost: &mut RestCost,
) {
    let Ctx {
        state, node, chain, ..
    } = c;
    // a payment of this wallet's own, still waiting, is not someone else's to hold
    let own: Vec<String> = w
        .pending
        .iter()
        .flat_map(|p| outputs_of(p, my_trees))
        .map(|x| x.0)
        .collect();
    let own_tx: Vec<String> = w.pending.iter().map(|p| p.tx.clone()).collect();
    for id in ids {
        if w.boxes.contains_key(id)
            || w.expect.contains(id)
            || w.void.contains(id)
            || own.contains(id)
        {
            continue;
        }
        let (found, answered) = pending_payment(nodes, id);
        // a node showed such a payment, held or not: then nobody can say there is none
        let offered = found.is_some();
        let mut held = false;
        if let Some(tx) = found.filter(|t| !own_tx.contains(&String::from(t.id()))) {
            let coins: Vec<String> = tx.inputs.iter().map(|i| String::from(i.box_id)).collect();
            let reads: Vec<String> = tx
                .data_inputs
                .as_ref()
                .map(|d| d.iter().map(|i| String::from(i.box_id)).collect())
                .unwrap_or_default();
            let all: Vec<String> = coins.iter().chain(reads.iter()).cloned().collect();
            let mut seen_at = 0u64;
            let checked = prove_any_agreed(node, chain, agreed_height(state), &all, cost).map_err(|e| format!("its coins could not be proven ({e})")).and_then(|(h, f)| {
                seen_at = h as u64;
                let parse = |v: &[Option<Vec<u8>>]| v.iter().map(|b| b.as_ref().ok_or("a coin it spends is not an unspent box (spent, or made by a payment not yet in a block)".to_string()).and_then(|b| ErgoBox::sigma_parse_bytes(b).map_err(|e| format!("box decode: {e:?}")))).collect::<Result<Vec<_>, String>>();
                let (fi, fd) = f.split_at(coins.len());
                crate::proofs::verifies(&tx, parse(fi)?, parse(fd)?, chain)
            });
            match checked
                .and_then(|_| {
                    tx.sigma_serialize_bytes()
                        .map(|b| hex(&b))
                        .map_err(|e| format!("{e:?}"))
                })
                .and_then(|hexs| hold(state, w, my_trees, &hexs, &coins, seen_at))
            {
                Ok(_) => {
                    held = true;
                    say!("  {id}  a node holds the payment that makes it, not yet in a block; its coins are unspent and its signatures verify: taken and held")
                }
                Err(why) => {
                    say!("  {id}  a node holds a payment that makes it, but it is not held: {why}")
                }
            }
        }
        crate::out::push(
            "pools",
            serde_json::json!({ "id": id, "answered": answered, "offered": offered, "held": held }),
        );
    }
}

/// Asks every node what it lists for each address, and adds what is not yet asked about: a node can hide a box
/// but cannot invent one, because everything listed is proven like a receipt. Handed back: whether every node
/// answered in full for every address, and whether any answered at all.
pub(super) fn listed_at(
    nodes: &[String],
    addr_strs: &[String],
    ids: &mut Vec<String>,
    cost: &mut RestCost,
) -> (bool, bool) {
    let (mut all_listed, mut any_listed) = (true, false);
    for a in addr_strs {
        for n in nodes {
            let (mut listed, mut offset) = (vec![], 0usize);
            loop {
                cost.requests += 1;
                cost.sent += a.len();
                let Ok(r) = http()
                    .post(&format!(
                        "{n}/blockchain/box/unspent/byAddress?offset={offset}&limit=200"
                    ))
                    .set("Content-Type", "application/json")
                    .send_string(a)
                else {
                    say!("reconcile: {n} did not answer");
                    all_listed = false;
                    break;
                };
                let text = r.into_string().unwrap_or_default();
                cost.received += text.len();
                any_listed = true;
                let page: Vec<String> = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.as_array().map(|x| {
                            x.iter()
                                .filter_map(|b| b["boxId"].as_str().map(String::from))
                                .collect()
                        })
                    })
                    .unwrap_or_default();
                let n_page = page.len();
                listed.extend(page);
                if n_page < 200 {
                    break;
                }
                // a node that lists more than this is not followed further, and what it listed is not taken for everything
                if offset >= 1800 {
                    say!("reconcile: {n} lists more than {} boxes for one address; only those are looked at", offset + 200);
                    all_listed = false;
                    break;
                }
                offset += 200;
            }
            let fresh: Vec<String> = listed
                .iter()
                .filter(|i| !ids.contains(i))
                .cloned()
                .collect();
            say!(
                "reconcile: {n} lists {} unspent boxes, {} not yet tracked",
                listed.len(),
                fresh.len()
            );
            ids.extend(fresh);
        }
    }
    (all_listed, any_listed)
}

/// The first proof a node gives of the boxes asked about, against the stored chain or a few work-checked headers
/// the node adds to it. Those headers are kept for the next proof; what the peers agreed on is not touched.
pub(super) fn proof_from(
    nodes: &[String],
    state: &str,
    chain: &mut Vec<Header>,
    agreed: u32,
    ids: &[String],
    cost: &mut RestCost,
) -> Option<Proven> {
    let mut proven = None;
    for n in nodes {
        if let Some(peer) = n.strip_prefix("p2p://") {
            match prove_p2p(peer, chain, ids, cost) {
                Ok(x) => {
                    proven = Some(x);
                    break;
                }
                Err(e) => {
                    say!("proof from {n}: {e}");
                    continue;
                }
            }
        }
        match prove_reading(n, chain, Some(agreed), ids, cost) {
            Ok(x) => {
                proven = Some(x);
                break;
            }
            Err(e) => {
                let added = extend_chain(n, chain, 3, cost);
                if added == 0 {
                    say!("proof from {n}: {e}");
                    continue;
                }
                say!("node is {added} block(s) past the agreed tip; extended with work-checked, linked headers from {n}");
                match prove_reading(n, chain, Some(agreed), ids, cost) {
                    Ok(x) => {
                        proven = Some(x);
                        break;
                    }
                    Err(e) => say!("proof from {n}: {e}"),
                }
            }
        }
    }
    if proven.is_some() {
        // headers this node added are kept for the next proof, but the file stays short and `agreed-tip.bin` is not
        // touched: what the peers agreed on is still what it was. However many a node has added, the agreed tip and the
        // few below it stay in the file, so a proof from an honest node at the agreed tip can still be read (`prove_reading`)
        let keep = chain
            .iter()
            .position(|h| h.height <= agreed)
            .map_or(16, |i| 16.max(i + crate::chain::FRESH));
        if let Err(e) = write_whole(
            &format!("{state}/agreed-chain.bin"),
            &chain_bytes(chain.iter().take(keep)),
        ) {
            say!(
                "note: the chain could not be saved in {state} ({e}); the one from before is kept"
            );
        }
    }
    proven
}

/// What became of the payments this wallet sent, read off the same proof as the balance (`payment_outcome`): each
/// one waiting is confirmed, given up, found to have its coins spent, or left waiting and sent again.
pub(super) fn settle_sent(
    c: &Ctx,
    w: &mut WalletFile,
    shown: &Shown,
    nodes: &[String],
    my_trees: &[ErgoTree],
    books: &mut Books,
    cost: &mut RestCost,
) -> Sent {
    let Shown {
        ids,
        found,
        height,
        agreed,
        settled,
        chain,
    } = *shown;
    let (node, expect) = (&c.node, &mut books.expect);
    const GIVE_UP: u64 = 10;
    // boxes that payments of this wallet (waiting or settled) spend, and every box a waiting payment creates
    let spent_by_us: Vec<String> = w
        .pending
        .iter()
        .flat_map(|p| p.inputs.iter())
        .chain(w.done.iter().flat_map(|d| d.inputs.iter()))
        .cloned()
        .collect();
    let mut made: Vec<String> = vec![];
    let mut refresh = false;
    let present = |id: &str| {
        ids.iter()
            .position(|x| x == id)
            .map(|i| found[i].is_some())
            .unwrap_or(false)
    };
    let mut still: Vec<Pending> = vec![];
    let mut done: Vec<Done> = std::mem::take(&mut w.done);
    // A payment given up on is still watched. If a box it creates is there, it was included after all. If a box it
    // spends is gone and none it creates is there, it may have been included and spent on: that is settled by a
    // proof that it is in the chain, or left as "coins spent".
    let late: Vec<(String, Option<u32>)> = done
        .iter()
        .filter(|d| {
            settled
                && d.result == "not included"
                && !present(&d.watch)
                && d.inputs.iter().any(|i| !present(i))
        })
        .map(|d| {
            (
                d.tx.clone(),
                prove_included(node, chain, &d.tx, cost)
                    .ok()
                    .filter(|h| *h <= agreed),
            )
        })
        .collect();
    for (tx, included) in late {
        let Some(d) = done.iter_mut().find(|d| d.tx == tx) else {
            continue;
        };
        match included {
            Some(h) => {
                d.result = "confirmed".into();
                d.seen_at = h as u64;
                d.other
                    .insert("proof".into(), serde_json::json!("inclusion"));
                say!("payment {tx}: included after all: proven to be in the block at height {h}");
            }
            None => {
                d.result = "coins spent".into();
                d.seen_at = height as u64;
                say!("payment {tx}: given up on earlier; its coins have since been spent, by it or by something else");
            }
        }
        crate::out::push(
            "payments",
            serde_json::json!({ "tx": tx, "result": d.result, "height": included, "proof": included.map(|_| "inclusion") }),
        );
        refresh = true;
    }
    for d in done
        .iter_mut()
        .filter(|d| settled && d.result == "not included")
    {
        if present(&d.watch) {
            d.result = "confirmed".into();
            d.seen_at = height as u64;
            crate::out::push(
                "payments",
                serde_json::json!({ "tx": d.tx, "result": "confirmed" }),
            );
            say!(
                "payment {}: included after all; confirmed at height {height} or before",
                d.tx
            );
        }
    }
    // A payment settled earlier as "coins spent" (or, by an older build, "spent elsewhere") is given one try at a proof
    // that it is in the chain. If it is, it confirmed: the earlier word is put right.
    for d in done.iter_mut().filter(|d| {
        settled
            && (d.result == "coins spent" || d.result == "spent elsewhere")
            && !d.other.contains_key("inclusion_tried")
    }) {
        match prove_included(node, chain, &d.tx, cost) {
            Ok(h) if h <= agreed => {
                d.result = "confirmed".into();
                d.seen_at = h as u64;
                d.other
                    .insert("proof".into(), serde_json::json!("inclusion"));
                d.other.insert("inclusion_tried".into(), true.into());
                crate::out::push(
                    "payments",
                    serde_json::json!({ "tx": d.tx, "result": "confirmed", "height": h, "proof": "inclusion" }),
                );
                say!(
                    "payment {}: it did confirm: proven to be in the block at height {h}",
                    d.tx
                );
            }
            Ok(_) => {}
            // a node that knows no such transaction has answered; one that could not be reached has not, and is asked again next time
            Err(e) => {
                if e.contains("knows no such transaction") {
                    d.other.insert("inclusion_tried".into(), true.into());
                }
            }
        }
    }
    for mut p in std::mem::take(&mut w.pending) {
        let tx = p.tx.clone();
        let inputs: Vec<String> = p.inputs.clone();
        let first = p.first_look.unwrap_or(height as u64);
        p.first_look = Some(first);
        let outs = outputs_of(&p, my_trees);
        let created = present(&p.watch) || outs.iter().any(|(id, _)| present(id));
        made.extend(outs.into_iter().map(|x| x.0));
        let verdict = payment_outcome(
            settled,
            created,
            inputs.iter().any(|i| !present(i)),
            (height as u64).saturating_sub(first) >= GIVE_UP,
        );
        // What a node says of a payment the proof cannot settle: in a block, or unknown to it. A node's word, kept
        // apart from the verdict and shown as such; it is asked only then.
        // First, whether it can be proven to be in the chain all the same: the block a node names, linked by hash to
        // the agreed chain, and a Merkle proof from the transaction to that block (`prove_included`). Then it confirmed,
        // whatever became of its boxes since.
        let included: Option<u32> = (verdict == Some("coins spent"))
            .then(|| prove_included(node, chain, &tx, cost).ok())
            .flatten()
            .filter(|h| *h <= agreed);
        let verdict = if included.is_some() {
            Some("confirmed")
        } else {
            verdict
        };
        let reported: Option<String> = (verdict == Some("coins spent"))
            .then(|| node_says(nodes, &tx))
            .flatten();
        match verdict {
            Some(v) => {
                crate::out::push(
                    "payments",
                    serde_json::json!({ "tx": tx, "result": v, "node_says": reported, "height": included, "proof": included.map(|_| "inclusion") }),
                );
                if v == "coins spent" || included.is_some() {
                    refresh = true;
                }
                say!("payment {tx}: {}", match v {
                    "confirmed" => match included { Some(h) => format!("confirmed: proven to be in the block at height {h} (what it created has been spent since)"), None => format!("confirmed at height {height} or before") },
                    "coins spent" => format!("its coins are spent and nothing it created is unspent now. Either it confirmed and what it paid has been spent since (by the payee, or by another wallet of this phrase), or another transaction spent its coins: the proof cannot tell which. {} Check before paying again", match reported.as_deref() {
                        Some(r) => format!("A node says: {r} (its word, not proven)."), None => "No node could say more.".to_string() }),
                    _ => format!("NOT INCLUDED after {GIVE_UP} blocks; its coins count as this wallet's again") });
                if v != "confirmed" || included.is_some() {
                    // nothing it would have created is expected any more
                    let outs: Vec<String> =
                        outputs_of(&p, my_trees).into_iter().map(|x| x.0).collect();
                    expect.retain(|e| !outs.contains(e));
                }
                let mut d = Done {
                    tx,
                    watch: p.watch.clone(),
                    inputs: p.inputs.clone(),
                    result: v.to_string(),
                    seen_at: included.unwrap_or(height) as u64,
                    ..Default::default()
                };
                if included.is_some() {
                    d.other
                        .insert("proof".into(), serde_json::json!("inclusion"));
                }
                if let Some(r) = &reported {
                    d.other.insert("node_says".into(), serde_json::json!(r));
                }
                done.insert(0, d);
            }
            None => {
                // (TACHYS_NO_RESEND is for tests of the not-included path, on a private network only)
                if height as u64 > first
                    && !(private_net() && std::env::var_os("TACHYS_NO_RESEND").is_some())
                {
                    if let (Some(n), Some(j)) = (
                        nodes.iter().find(|n| !n.starts_with("p2p://")),
                        Some(p.json.as_str()).filter(|j| !j.is_empty()),
                    ) {
                        let _ = http()
                            .post(&format!("{n}/transactions"))
                            .set("Content-Type", "application/json")
                            .send_string(j);
                    }
                }
                crate::out::push(
                    "payments",
                    serde_json::json!({ "tx": tx, "result": "waiting" }),
                );
                say!(
                    "payment {tx}: waiting, {} block(s) since first checked",
                    (height as u64).saturating_sub(first)
                );
                still.push(p);
            }
        }
    }
    // The last twenty settled payments are kept, and beyond them any payment given up as "not included" whose coins this
    // wallet still tracks: the next payment spends one of those coins (`cmd_send`), which is what keeps that payment and
    // the next from both confirming. Once its coins are spent or gone it ages out like the rest, so this stays bounded.
    let mut n = 0;
    done.retain(|d| {
        n += 1;
        n <= 20 || (d.result == "not included" && d.inputs.iter().any(|i| w.boxes.contains_key(i)))
    });
    // boxes of a payment still waiting are not this wallet's to count or spend again; boxes of a failed one are
    let held: Vec<String> = still
        .iter()
        .flat_map(|p| p.inputs.iter().cloned())
        .collect();
    let quiet: Vec<String> = still
        .iter()
        .flat_map(|p| p.inputs.iter().chain(std::iter::once(&p.watch)))
        .chain(
            done.iter()
                .flat_map(|d| d.inputs.iter().chain(std::iter::once(&d.watch))),
        )
        .cloned()
        .chain(made)
        .collect();
    w.pending = still;
    w.done = done;
    Sent {
        spent_by_us,
        held,
        quiet,
        refresh,
    }
}

/// A held payment none of whose boxes exist, one of whose coins this wallet had seen unspent and is now gone, at or
/// below the agreed tip, can never confirm: what it promised is void, and the payment is put aside (`given-up/`;
/// nothing sends it again). A coin never seen is not taken for gone: it may be made by a payment not yet in a block
/// (a payer who chains payments). Coins seen unspent now are written down with the payment.
pub(super) fn settle_held(
    c: &Ctx,
    shown: &Shown,
    promises: &[Held],
    books: &mut Books,
    cost: &mut RestCost,
) {
    let Shown {
        ids,
        found,
        height,
        agreed,
        settled,
        chain,
    } = *shown;
    let (node, state) = (&c.node, &c.state);
    let Books { expect, void, .. } = books;
    let present = |id: &str| {
        ids.iter()
            .position(|x| x == id)
            .map(|i| found[i].is_some())
            .unwrap_or(false)
    };
    for (file, v, inputs, outs) in promises {
        if !settled || outs.iter().any(|o| present(o)) {
            continue;
        }
        // each coin with the height it was seen unspent at: a proof from below that height (a node behind) says nothing
        // about whether it is gone since
        let mut seen: serde_json::Map<String, serde_json::Value> =
            v["coins_seen"].as_object().cloned().unwrap_or_default();
        let fresh: Vec<String> = inputs
            .iter()
            .filter(|i| present(i) && !seen.contains_key(*i))
            .cloned()
            .collect();
        if !fresh.is_empty() {
            for f in fresh {
                seen.insert(f, (height as u64).into());
            }
            let mut v = v.clone();
            v["coins_seen"] = serde_json::Value::Object(seen.clone());
            let _ = write_whole(&file.to_string_lossy(), v.to_string().as_bytes());
        }
        let seen_below = |c: &String| {
            seen.get(c)
                .and_then(|h| h.as_u64())
                .is_some_and(|h| h <= height as u64)
        };
        let aside = format!("{state}/given-up");
        // put aside as void, and its coins are all back (the payment that spent one was itself undone): held again
        if file.starts_with(&aside)
            && outs.iter().all(|o| void.contains(o))
            && inputs.iter().all(|i| present(i))
        {
            void.retain(|o| !outs.contains(o));
            for o in outs {
                if !expect.contains(o) {
                    expect.push(o.clone());
                }
            }
            if let Some(name) = file.file_name() {
                let _ = std::fs::create_dir_all(format!("{state}/outbox"));
                let _ = std::fs::rename(
                    file,
                    std::path::Path::new(&format!("{state}/outbox")).join(name),
                );
            }
            say!("  a payment held for this wallet that was void has its coins back (what spent one was undone): it is held again");
            continue;
        }
        let Some(gone) = inputs.iter().find(|i| !present(i) && seen_below(i)) else {
            continue;
        };
        if void.iter().any(|o| outs.contains(o)) {
            continue;
        }
        // a coin gone and no box of it there is also what a payment that confirmed, and whose boxes were spent since,
        // looks like: if it can be proven to be in the chain, it is not void
        // the id read from the payment's own bytes, never from the file's word
        if let Some(tx) = v["tx_bytes"]
            .as_str()
            .and_then(unhex)
            .and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok())
            .map(|t| String::from(t.id()))
        {
            if let Ok(h) = prove_included(node, chain, &tx, cost) {
                if h <= agreed {
                    say!("  a payment held for this wallet is in the chain (proven, block {h}); what it made has been spent since");
                    continue;
                }
            }
        }
        expect.retain(|e| !outs.contains(e));
        for o in outs {
            if !void.contains(o) {
                void.push(o.clone());
            }
        }
        let _ = std::fs::create_dir_all(&aside);
        if let Some(name) = file.file_name() {
            if !file.starts_with(&aside) {
                let _ = std::fs::rename(file, std::path::Path::new(&aside).join(name));
            }
        }
        crate::out::push(
            "void",
            serde_json::json!({ "boxes": outs, "coin_gone": gone, "reason": "a coin it spends, seen unspent before, is gone" }),
        );
        say!("  a payment held for this wallet can never confirm: its coin {}, seen unspent before, is not an unspent box at height {height}; what it promised ({}) is void", clip(gone, 16), outs.iter().map(|o| clip(o, 16)).collect::<Vec<_>>().join(", "));
    }
}

/// Each box asked about, read off the proof: kept, added, dropped as spent, or said to be undone, void or absent.
/// Handed back: whether a box went that no payment of this wallet spent (another wallet of the phrase may be at work).
pub(super) fn settle_boxes(
    c: &Ctx,
    w: &mut WalletFile,
    shown: &Shown,
    sent: &Sent,
    books: &mut Books,
    my_trees: &[ErgoTree],
    proven_held: &[String],
) -> bool {
    let Shown {
        ids,
        found,
        height,
        agreed,
        settled,
        chain,
    } = *shown;
    let Sent {
        spent_by_us,
        held,
        quiet,
        ..
    } = sent;
    let Books {
        expect,
        void,
        undone,
    } = books;
    let sub = c.sub;
    let mut refresh = false;
    // each box asked about, for a program: its id, what the proof showed ("proven": unspent and ours, at or below the agreed
    // tip; "seen": the same above it; "absent"; "spent"; "undone"; "void"; "expected"; "other_address"; "undecodable"; "absent_above_agreed_tip";
    // "absent_below_confirmed")
    let checked = |id: &str, state: &str| {
        crate::out::push(
            "checked",
            serde_json::json!({ "id": id, "state": state, "height": height }),
        )
    };
    // the boxes paying this wallet that its own payments still waiting make: built and signed here from coins proven
    // unspent under an agreed tip, which this sync has asked about again (a coin of one gone has settled it, above)
    let own_waiting: Vec<String> = w
        .pending
        .iter()
        .flat_map(|p| outputs_of(p, my_trees))
        .filter(|x| x.1)
        .map(|x| x.0)
        .collect();
    // a box confirmed under a header the agreed chain no longer has there left with its block: undone, not spent.
    // Only the stored chain can say, so a reorganisation further back than it reaches still reads as a spend.
    let header_at = |h: u64| {
        chain
            .iter()
            .find(|x| x.height as u64 == h)
            .map(|x| hex(&crate::chain::id32(x)))
    };
    let boxes = &mut w.boxes;
    for (id, f) in ids.iter().zip(found.iter()) {
        if held.contains(id) {
            continue;
        }
        match f.as_ref().map(|b| ErgoBox::sigma_parse_bytes(b)) {
            // a box one of this wallet's own payments spent or made: nothing to say on a sync, but a receipt asked about by name gets its answer
            None if quiet.contains(id) && !boxes.contains_key(id) && !expect.contains(id) => {
                if sub == "receive" {
                    checked(id, "spent");
                    say!(
                        "  {id}  not unspent at height {height}: a payment of this wallet spent it"
                    );
                }
            }
            Some(Ok(b)) if !my_trees.contains(&b.ergo_tree) && quiet.contains(id) => {}
            None if !settled && boxes.contains_key(id) => {
                checked(id, "absent_above_agreed_tip");
                say!("  {id}  absent at height {height}, above the tip the peers agree on ({agreed}): kept until they reach it")
            }
            // A proof can be against a stored header below the one this box was confirmed at (a node a block or two behind):
            // there the box did not exist yet, and its absence says nothing of its being spent, nor of its block having left
            // the chain (`undone`, below): that is read from a proof at or above the height it was confirmed at.
            None if boxes
                .get(id)
                .and_then(|b| b.confirmed_at)
                .is_some_and(|c| c > height as u64) =>
            {
                let c = boxes.get(id).and_then(|b| b.confirmed_at).unwrap_or(0);
                checked(id, "absent_below_confirmed");
                say!("  {id}  absent at height {height}, below where it was confirmed ({c}): kept")
            }
            None => match boxes.remove(id) {
                Some(old)
                    if settled
                        && !spent_by_us.contains(id)
                        && !old.confirmed_in.is_empty()
                        && old
                            .confirmed_at
                            .and_then(header_at)
                            .is_some_and(|now| now != old.confirmed_in) =>
                {
                    checked(id, "undone");
                    refresh = true;
                    if !undone.contains(id) {
                        undone.push(id.clone());
                    }
                    say!("  {id}  gone at height {height}, and the block it was confirmed under (height {}) is no longer in the chain the peers agree on: UNDONE by a reorganisation. It may be included again, or its coins may have been spent elsewhere; until it is proven again it is not a payment", old.confirmed_at.unwrap_or(0));
                }
                Some(_) => {
                    checked(id, "spent");
                    if !spent_by_us.contains(id) {
                        refresh = true;
                    }
                    say!(
                        "  {id}  gone at height {height}: spent{}; removed",
                        if spent_by_us.contains(id) {
                            ""
                        } else {
                            " (not by a payment of this wallet)"
                        }
                    );
                }
                None if expect.contains(id) => {
                    crate::out::push(
                        "checked",
                        serde_json::json!({ "id": id, "state": "expected", "height": height, "held_proven": proven_held.contains(id), "own": own_waiting.contains(id) }),
                    );
                    say!("  {id}  expected from our own payment, not on chain yet at {height}");
                }
                // an undone box stays undone, asked about again, until it is back or its coins are spent
                None if undone.contains(id) => {
                    checked(id, "undone");
                    say!("  {id}  undone by a reorganisation and not back in the chain at height {height}: not a payment now");
                }
                None if void.contains(id) => {
                    checked(id, "void");
                    say!("  {id}  VOID: the payment that was to make it can never confirm (a coin it spends is gone, or it was refused for good): not accepted");
                }
                None => {
                    checked(id, "absent");
                    say!("  {id}  ABSENT at height {height}: not accepted");
                }
            },
            Some(Err(_)) => {
                checked(id, "undecodable");
                say!("  {id}  present but undecodable: not accepted")
            }
            Some(Ok(b)) if !my_trees.contains(&b.ergo_tree) => {
                checked(id, "other_address");
                say!("  {id}  exists but pays another address: not accepted")
            }
            Some(Ok(b)) => {
                // a box that was undone and is back is the same payment, not a new one
                let back = undone.contains(id);
                let new = !boxes.contains_key(id) && !back;
                undone.retain(|u| u != id);
                void.retain(|v| v != id);
                // the first height at or below an agreed tip at which this box was proven; confirmations are counted from it (a lower bound)
                let confirmed_at: Option<u64> = boxes
                    .get(id)
                    .and_then(|x| x.confirmed_at)
                    .or((height <= agreed).then_some(height as u64));
                let confirmed_in: String = boxes
                    .get(id)
                    .filter(|x| x.confirmed_at.is_some())
                    .map(|x| x.confirmed_in.clone())
                    .filter(|c| !c.is_empty())
                    .or_else(|| confirmed_at.and_then(header_at))
                    .unwrap_or_default();
                let confirmations =
                    confirmed_at.map_or(0, |c| (agreed as u64 + 1).saturating_sub(c));
                let first_proven = boxes
                    .get(id)
                    .map(|x| x.first_proven)
                    .filter(|h| *h > 0)
                    .unwrap_or(height as u64);
                boxes.insert(
                    id.clone(),
                    TrackedBox {
                        value: *b.value.as_u64(),
                        tokens: b.tokens.as_ref().map_or(0, |t| t.len()),
                        assets: b
                            .tokens
                            .iter()
                            .flat_map(|t| t.iter())
                            .map(|x| Asset {
                                id: String::from(x.token_id),
                                amount: *x.amount.as_u64(),
                            })
                            .collect(),
                        proven_at: height as u64,
                        first_proven,
                        confirmed_at,
                        confirmed_in,
                        bytes: hex(f.as_ref().expect("present")),
                        ..Default::default()
                    },
                );
                if back {
                    say!("  {id}  back in the chain after it was undone: the same payment, not a new one");
                }
                expect.retain(|e| e != id);
                crate::out::push(
                    "checked",
                    serde_json::json!({ "id": id, "state": if confirmed_at.is_some() { "proven" } else { "seen" }, "height": height, "nanoerg": b.value.as_u64(), "confirmations": confirmations, "added": new }),
                );
                // a box confirmed on an earlier sync stays confirmed when this proof happens to be against a block above
                // the agreed tip: it was proven under one then and is still unspent now
                if confirmed_at.is_some() {
                    say!("  {id}  {} nanoERG, proven unspent at {height}, confirmations {confirmations}{}", b.value.as_u64(), if new { " (added)" } else { "" });
                } else {
                    say!("  {id}  {} nanoERG, seen unspent at {height}, above the tip the peers agree on ({agreed}): one source, confirmations {confirmations}{}", b.value.as_u64(), if new { " (added)" } else { "" });
                }
            }
        }
    }
    refresh
}

/// wallet receive | status, and the second half of restore: prove the wallet's boxes against the agreed chain and
/// say what became of the payments it sent. The steps, in order: what to ask about (payments held from a node's pool
/// with `--pools`, what the nodes list with `--reconcile`); one proof of all of it (`proof_from`); then the
/// wallet's own payments (`settle_sent`), payments held for it (`settle_held`) and its boxes (`settle_boxes`) are
/// each read off that proof.
pub(super) fn cmd_sync(c: &Ctx, o: Open) -> Ended {
    let Ctx {
        args,
        state,
        node,
        chain,
        enc,
        ..
    } = c;
    let sub = c.sub;
    let Open {
        mut w,
        addr,
        scanning,
        my_trees,
        addr_strs,
        ..
    } = o;
    if node.is_empty() || chain.is_empty() {
        return Err(crate::out::Fail::new(2, "usage: tachys wallet receive|status|restore --state DIR --node URL[,URL...] [--ids ID,...] [--reconcile] [--mnemonic-file FILE]  (run `tachys tips --state DIR` first)".to_string()));
    }
    let nodes: Vec<String> = crate::nodes::each(node);
    let mut cost = RestCost::default();
    let mut tracked: Vec<String> = w.boxes.keys().cloned().collect();
    let mut ids: Vec<String> = match sub {
        "receive" => arg(args, "--ids", String::new())?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        "status" | "restore" | "watch" => {
            // boxes we created ourselves (change from our own payments) are looked for without asking anyone
            let mut v = tracked.clone();
            v.extend(w.expect.iter().cloned());
            // boxes undone by a reorganisation are asked about until they are back (or seen no more for long: the last 64)
            v.extend(
                w.undone
                    .iter()
                    .filter(|u| !v.contains(u))
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            v
        }
        _ => {
            return Err(crate::out::Fail::new(
                2,
                "usage: tachys wallet new|restore|receive|status|send ...".to_string(),
            ))
        }
    };
    // every box a waiting payment creates is asked about too: any one of them existing shows it is in the chain
    for p in &w.pending {
        for (id, _) in outputs_of(p, &my_trees) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    if sub == "receive" && args.iter().any(|a| a == "--pools") {
        take_from_pools(c, &mut w, &my_trees, &nodes, &ids, &mut cost);
    }
    // payments still waiting, and ones given up on (they may yet be included): the boxes they spend and one they create.
    // On every sync, `receive` too: each such payment is settled by this proof, and a coin of it not asked about would
    // read as gone
    let watched = w.pending.iter().map(|p| (&p.inputs, &p.watch)).chain(
        w.done
            .iter()
            .filter(|d| d.result == "not included")
            .map(|d| (&d.inputs, &d.watch)),
    );
    for id in watched.flat_map(|(inputs, watch)| inputs.iter().chain(std::iter::once(watch))) {
        if !id.is_empty() && !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    // payments this wallet holds that someone else made to it (`wallet promise`, or fetched from a node's pool with
    // `--pools`): the boxes each promises and the coins it spends are asked about too, so that one whose coin is gone
    // is known to be dead (void) and not waited for
    let promises = held_promises(state, &w);
    // the boxes promised by a payment held with its coins proven unspent and its signatures verified when it was taken
    // (`--pools`); one handed over (`wallet promise`) was checked for nothing, and a file from before says nothing
    let proven_held: Vec<String> = promises
        .iter()
        .filter(|p| p.1["proven"] == true)
        .flat_map(|p| p.3.iter().cloned())
        .collect();
    for (_, _, inputs, outs) in &promises {
        for id in inputs.iter().chain(outs.iter()) {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    // `refresh`: an earlier sync saw the sign of another wallet of this phrase at work (a box gone that no payment of
    // ours spent, or a payment whose outcome the proof could not tell), so the nodes are asked again what the addresses hold
    let reconcile = sub == "restore"
        || (sub == "status" && (args.iter().any(|a| a == "--reconcile") || w.refresh));
    if reconcile && w.refresh && !args.iter().any(|a| a == "--reconcile") {
        say!("looking again for this wallet's boxes: something else has spent or moved its coins since the last sync");
    }
    // every node given is asked what it lists for each address, and the union is taken
    let (all_listed, any_listed) = if reconcile {
        listed_at(&nodes, &addr_strs, &mut ids, &mut cost)
    } else {
        (false, false)
    };
    crate::out::set("listed_by_all", reconcile.then_some(all_listed));
    // nothing known and nobody to ask is not "a balance of nothing"
    if ids.is_empty() && reconcile && !any_listed {
        return Err("no node answered what this wallet's addresses hold".into());
    }
    if ids.is_empty() {
        // a restored wallet that every node says is empty at all twenty addresses keeps the first
        if scanning && all_listed {
            w.scan_addresses = false;
            let keep = (w.addresses.unwrap_or(1) as usize).clamp(1, addr_strs.len());
            w.addresses = Some(keep as u64);
            w.address_list = Some(addr_strs.iter().take(keep).cloned().collect());
            c.save(&w)?;
        }
        crate::out::set("address", enc.address_to_str(&addr));
        crate::out::set("addresses", &addr_strs);
        crate::out::set("nanoerg", 0);
        crate::out::set("boxes", 0);
        crate::out::set("tokens", serde_json::json!([]));
        say!(
            "address {}\nno boxes tracked; balance 0 ERG",
            enc.address_to_str(&addr)
        );
        return Ok(());
    }
    let mut chain = chain.clone();
    // the tip the peers agreed on: a proof at or below it settles (below); one above it is only seen
    let agreed = agreed_height(state);
    let proven = proof_from(&nodes, state, &mut chain, agreed, &ids, &mut cost);
    let Some((height, found)) = proven else {
        say!("NOT PROVEN by any node given");
        return Err(crate::out::Fail::said(1));
    };
    // A box counts as confirmed once it has been proven at or below the tip the peers agreed on. A proof against a
    // newer header rests on that header's one source and its own work: seen, not confirmed.
    // The same goes for what is taken away. A box is dropped, and a payment called confirmed or failed, only on a proof
    // at or below the agreed tip; a proof above it can add what it shows (marked as seen) and settles nothing.
    let settled = height <= agreed;
    let shown = Shown {
        ids: &ids,
        found: &found,
        height,
        agreed,
        settled,
        chain: &chain,
    };
    let mut books = Books {
        expect: w.expect.clone(),
        void: std::mem::take(&mut w.void),
        undone: std::mem::take(&mut w.undone),
    };
    let sent = settle_sent(c, &mut w, &shown, &nodes, &my_trees, &mut books, &mut cost);
    settle_held(c, &shown, &promises, &mut books, &mut cost);
    let refresh = settle_boxes(
        c,
        &mut w,
        &shown,
        &sent,
        &mut books,
        &my_trees,
        &proven_held,
    ) | sent.refresh;
    let boxes = &w.boxes;
    tracked = boxes.keys().cloned().collect();
    let total: u64 = boxes.values().fold(0u64, |a, b| a.saturating_add(b.value));
    // a restored wallet has now been looked over at all twenty addresses: keep those up to the last one that holds a box
    let in_use: Option<usize> = (scanning && all_listed).then(|| {
        let trees: Vec<_> = boxes
            .values()
            .filter_map(|b| unhex(&b.bytes))
            .filter_map(|b| ErgoBox::sigma_parse_bytes(&b).ok())
            .map(|b| b.ergo_tree)
            .collect();
        my_trees
            .iter()
            .rposition(|t| trees.contains(t))
            .map_or(1, |i| i + 1)
    });
    let Books {
        expect,
        mut undone,
        mut void,
    } = books;
    w.expect = expect;
    let keep_last = |v: &mut Vec<String>| {
        let n = v.len();
        if n > 64 {
            v.drain(..n - 64);
        }
    };
    keep_last(&mut undone);
    keep_last(&mut void);
    (w.undone, w.void) = (undone, void);
    // asked every node and got an answer from each: the wallet's boxes are up to date, whoever moved them
    if reconcile && all_listed {
        w.refresh = false;
    }
    // (a sync that has just asked every node already holds what they list: nothing more to look for)
    if refresh && !(reconcile && all_listed) {
        w.refresh = true;
        say!("another wallet of this phrase may be in use, or a payment's outcome is unclear: the next sync asks the nodes again what this wallet's addresses hold");
    }
    crate::out::set("refresh_due", w.refresh);
    if let Some(n) = in_use.map(|n| {
        n.max(w.addresses.unwrap_or(1) as usize)
            .min(addr_strs.len())
    }) {
        w.scan_addresses = false;
        w.addresses = Some(n as u64);
        w.address_list = Some(addr_strs.iter().take(n).cloned().collect());
        if n > 1 {
            say!("addresses: {n} of this phrase hold coins or come before one that does; all {n} are kept");
        }
    }
    c.save(&w)?;
    crate::out::set("address", enc.address_to_str(&addr));
    crate::out::set("addresses", &addr_strs);
    crate::out::set("nanoerg", total);
    crate::out::set("boxes", tracked.len());
    // of that total, what is in boxes confirmed under an agreed tip (the rest is seen in a block one node has shown, and is not spent)
    crate::out::set(
        "confirmed_nanoerg",
        w.boxes
            .values()
            .filter(|b| b.confirmed_at.is_some())
            .fold(0u64, |a, b| a.saturating_add(b.value)),
    );
    crate::out::set(
        "tokens",
        w.tokens()
            .iter()
            .map(|(id, amount)| serde_json::json!({ "id": id, "amount": amount }))
            .collect::<Vec<_>>(),
    );
    crate::out::set("proven_at", height);
    crate::out::set("agreed_height", agreed);
    crate::out::set("settled", settled);
    crate::out::set("expecting", &w.expect);
    crate::out::set(
        "cost",
        serde_json::json!({ "requests": cost.requests, "sent": cost.sent, "received": cost.received }),
    );
    say!("address {}\nbalance {} ERG in {} boxes, proven at height {height}\ncost: {} requests, {} B sent, {} B received", enc.address_to_str(&addr), crate::util::erg(total), tracked.len(), cost.requests, cost.sent, cost.received);
    if height > agreed {
        say!("note: height {height} is above the tip the peers agree on ({agreed}); the headers between came from this node alone");
    }
    Ok(())
}

/// What became of a payment this wallet sent, from one proof at or below the agreed tip (`settled`; a proof above it
/// settles nothing):
///  - a box it creates exists (`created`): it is in the chain;
///  - a box it spends is gone (`input_gone`) and none it creates is there: "coins spent". The boxes it created may
///    have been spent already (a payee who moves the coins on, a miner who collects the fee, another wallet of the
///    same phrase spending the change), or another transaction may have spent its coins. A proof of what is unspent
///    now cannot tell these apart, and this wallet does not assume it is the only one holding its keys; saying
///    "failed" would invite paying twice. A node may be asked what it knows (`node_says`), as its word;
///  - neither, and `gave_up` (enough blocks since first looked): not included; its coins count as ours again (if it
///    is included later after all, that is noticed and it becomes confirmed);
///  - otherwise `None`: still waiting, and it is sent again in case the node dropped it.
pub(crate) fn payment_outcome(
    settled: bool,
    created: bool,
    input_gone: bool,
    gave_up: bool,
) -> Option<&'static str> {
    if !settled {
        None
    } else if created {
        Some("confirmed")
    } else if input_gone {
        Some("coins spent")
    } else if gave_up {
        Some("not included")
    } else {
        None
    }
}

/// What the first indexed node that answers says of a transaction: "in a block at height H", or that it does not
/// know it. `None` if no node answered. Nothing is decided on this.
pub(super) fn node_says(nodes: &[String], tx: &str) -> Option<String> {
    let mut unknown = false;
    for n in nodes.iter().filter(|n| !n.starts_with("p2p://")) {
        match http()
            .get(&format!("{n}/blockchain/transaction/byId/{tx}"))
            .call()
        {
            Ok(r) => {
                let v: serde_json::Value =
                    serde_json::from_str(&r.into_string().unwrap_or_default())
                        .unwrap_or(serde_json::Value::Null);
                if v["id"].as_str() == Some(tx) {
                    if let Some(h) = v["inclusionHeight"].as_u64() {
                        return Some(format!("it is in a block, at height {h}"));
                    }
                }
            }
            // one node not knowing it is not every node not knowing it: the next is asked, and this is said only if none does
            Err(ureq::Error::Status(404, _)) => unknown = true,
            Err(_) => {}
        }
    }
    unknown.then(|| "none asked knows such a transaction in a block".to_string())
}

/// The boxes a remembered payment creates, from its signed transaction: each id, and whether it pays this wallet.
pub(super) fn outputs_of(p: &Pending, my_trees: &[ErgoTree]) -> Vec<(String, bool)> {
    let mine: Vec<String> = my_trees
        .iter()
        .filter_map(|t| t.sigma_serialize_bytes().ok())
        .map(|b| hex(&b))
        .collect();
    serde_json::from_str::<serde_json::Value>(&p.json)
        .ok()
        .and_then(|j| j["outputs"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|o| {
            Some((
                o["boxId"].as_str()?.to_string(),
                o["ergoTree"]
                    .as_str()
                    .is_some_and(|t| mine.iter().any(|m| m == t)),
            ))
        })
        .collect()
}

/// Remember a payment this wallet sent, so later syncs can say what became of it: every box it spends, one box it
/// creates (if that box exists, the payment is in the chain), and the signed transaction to send again if needed.
pub(crate) fn note_pending(
    w: &mut WalletFile,
    tx: &ergo_lib::chain::transaction::Transaction,
    json: &str,
) {
    w.pending.push(Pending {
        tx: String::from(tx.id()),
        inputs: tx.inputs.iter().map(|i| String::from(i.box_id)).collect(),
        watch: tx
            .outputs
            .iter()
            .next()
            .map(|o| String::from(o.box_id()))
            .unwrap_or_default(),
        json: json.to_string(),
        ..Default::default()
    });
}
