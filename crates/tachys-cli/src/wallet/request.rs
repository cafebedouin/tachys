//! `wallet request`: a signing request someone else built is read, said in the wallet's own words, and signed if told to.

use super::*;

/// wallet request: say what a signing request does, and sign it if told to. A refusal is handed back, not printed:
/// nothing here stops the process.
pub(super) fn cmd_request(c: &Ctx, o: Open) -> Ended {
    let Ctx {
        args,
        state,
        node,
        chain,
        enc,
        ..
    } = c;
    let Open {
        mut w,
        phrase,
        no_phrase,
        n_addr,
        my_addrs,
        my_trees,
        ..
    } = o;
    // A signing request (ErgoPay, EIP-20, static form): someone else built the transaction; this wallet
    // says what it does, in its own words from the transaction itself, and signs only when told to.
    use base64::Engine;
    use ergo_lib::chain::transaction::reduced::ReducedTransaction;
    use ergo_lib::wallet::Wallet;
    let code: String = arg(args, "--code", String::new())?;
    let Some(b64) = code.trim().strip_prefix("ergopay:") else {
        return Err("not an ergopay: request".into());
    };
    if b64.starts_with("//") {
        return Err("this is a request to fetch from a server; only requests that carry the transaction are supported".into());
    }
    let Some(reduced) = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64.trim_end_matches('='))
        .ok()
        .and_then(|b| ReducedTransaction::sigma_parse_bytes(&b).ok())
    else {
        return Err("the request does not hold a transaction".into());
    };
    let tx = reduced.unsigned_tx.clone();
    // the wallet's own record of a box is used only if its bytes hash to the id they are filed under
    let tracked: std::collections::BTreeMap<String, ErgoBox> = w
        .boxes
        .iter()
        .filter_map(|(k, b)| {
            unhex(&b.bytes)
                .and_then(|x| ErgoBox::sigma_parse_bytes(&x).ok())
                .filter(|x| String::from(x.box_id()) == *k)
                .map(|x| (k.clone(), x))
        })
        .collect();
    let in_ids: Vec<String> = tx.inputs.iter().map(|i| String::from(i.box_id)).collect();
    // What each input needs in order to be spent, as the request states it. An input whose condition names one of
    // this wallet's keys is this wallet's money wherever it sits: an offer box it made, say, not only a box at its address.
    use ergotree_ir::sigma_protocol::sigma_boolean::{ProveDlog, SigmaBoolean};
    let my_keys: Vec<ProveDlog> = my_addrs
        .iter()
        .filter_map(|a| {
            if let Address::P2Pk(k) = a {
                Some(k.clone())
            } else {
                None
            }
        })
        .collect();
    let conditions: Vec<SigmaBoolean> = reduced
        .reduced_inputs()
        .iter()
        .map(|r| r.sigma_prop.clone())
        .collect();
    let needs_my_key = |id: &String| {
        in_ids
            .iter()
            .position(|x| x == id)
            .and_then(|i| conditions.get(i))
            .is_some_and(|c| names(c, &my_keys))
    };
    let mine: Vec<&ErgoBox> = in_ids.iter().filter_map(|i| tracked.get(i)).collect();
    let others: Vec<String> = in_ids
        .iter()
        .filter(|i| !tracked.contains_key(*i))
        .cloned()
        .collect();
    let mut mine: Vec<ErgoBox> = mine.into_iter().cloned().collect();
    // inputs that are not ours (a pool, a bank, another party) must be real: proven unspent now
    let mut other_boxes: Vec<ErgoBox> = vec![];
    if !others.is_empty() {
        if node.is_empty() || chain.is_empty() {
            return Err("it spends boxes that are not ours; a node and an agreed tip are needed to check them".into());
        }
        let mut cost = RestCost::default();
        match prove_any(node, chain, &others, &mut cost) {
            Err(e) => return Err(e.into()),
            Ok((_, found)) => {
                for (id, f) in others.iter().zip(found.iter()) {
                    // whose money an input is depends on who can spend it, not on whether this wallet had seen it:
                    // a box at our own address that we were not tracking yet is still ours
                    match f.as_ref().and_then(|b| ErgoBox::sigma_parse_bytes(b).ok()) {
                        Some(b) if my_trees.contains(&b.ergo_tree) || needs_my_key(id) => {
                            mine.push(b)
                        }
                        Some(b) => other_boxes.push(b),
                        None => {
                            return Err(
                                format!("input {}… is not an unspent box", clip(id, 16)).into()
                            )
                        }
                    }
                }
            }
        }
    }
    if mine.is_empty() {
        return Err("it spends nothing from this wallet".into());
    }
    // totals, from the transaction itself
    type Sums = (u64, std::collections::BTreeMap<String, u64>);
    fn add(s: &mut Sums, value: u64, tokens: Option<&ergotree_ir::chain::ergo_box::BoxTokens>) {
        // (whoever wrote the request chose these amounts: sums that would wrap are held at the top, as `leaves_wallet` holds them)
        s.0 = s.0.saturating_add(value);
        for t in tokens.into_iter().flat_map(|ts| ts.iter()) {
            let n = s.1.entry(String::from(t.token_id)).or_default();
            *n = n.saturating_add(*t.amount.as_u64());
        }
    }
    let fee_tree = fee_tree_hex();
    let (mut give, mut get, mut to_others, mut from_others): (Sums, Sums, Sums, Sums) =
        Default::default();
    let mut paid_to: Vec<String> = vec![];
    let mut fee = 0u64;
    for b in &mine {
        add(&mut give, *b.value.as_u64(), b.tokens.as_ref());
    }
    for b in &other_boxes {
        add(&mut from_others, *b.value.as_u64(), b.tokens.as_ref());
    }
    for o in tx.output_candidates.iter() {
        let tree_hex = o
            .ergo_tree
            .sigma_serialize_bytes()
            .map(|b| hex(&b))
            .unwrap_or_default();
        if my_trees.contains(&o.ergo_tree) {
            add(&mut get, *o.value.as_u64(), o.tokens.as_ref());
        } else if tree_hex == fee_tree {
            fee += *o.value.as_u64();
        } else {
            add(&mut to_others, *o.value.as_u64(), o.tokens.as_ref());
            let a = Address::recreate_from_ergo_tree(&o.ergo_tree)
                .map(|a| enc.address_to_str(&a))
                .unwrap_or_else(|_| format!("script {}", clip(&tree_hex, 40)));
            if !paid_to.contains(&a) {
                paid_to.push(a);
            }
        }
    }
    // a token that leaves the wallet and comes straight back is not part of what the request does to you
    let through: std::collections::BTreeMap<String, u64> = give
        .1
        .iter()
        .filter_map(|(id, a)| get.1.get(id).map(|b| (id.clone(), *a.min(b))))
        .collect();
    for (id, t) in &through {
        for side in [&mut give.1, &mut get.1] {
            if let Some(v) = side.get_mut(id) {
                *v -= t;
            }
        }
    }
    give.1.retain(|_, v| *v > 0);
    get.1.retain(|_, v| *v > 0);
    let show = |s: &Sums| -> String {
        format!(
            "{} nanoERG{}",
            s.0,
            s.1.iter()
                .map(|(id, a)| format!(", {a} of token {id}"))
                .collect::<String>()
        )
    };
    let sums = |s: &Sums| serde_json::json!({ "nanoerg": s.0, "tokens": s.1 });
    crate::out::set("give", sums(&give));
    crate::out::set("get", sums(&get));
    crate::out::set("others_put_in", sums(&from_others));
    crate::out::set("others_receive", sums(&to_others));
    crate::out::set("fee_nanoerg", fee);
    crate::out::set("paid_to", &paid_to);
    crate::out::set(
        "inputs",
        serde_json::json!({ "all": in_ids.len(), "yours": mine.len(), "others": other_boxes.len() }),
    );
    crate::out::set("net_nanoerg", get.0 as i128 - give.0 as i128);
    crate::out::set("signed", false);
    say!(
        "request: {} inputs ({} yours, {} proven others), {} outputs",
        in_ids.len(),
        mine.len(),
        other_boxes.len(),
        tx.output_candidates.len()
    );
    say!("you give: {}", show(&give));
    say!("you get back: {}", show(&get));
    say!("others put in: {}", show(&from_others));
    say!("others receive: {}", show(&to_others));
    say!("network fee: {fee} nanoERG");
    let net_erg = get.0 as i128 - give.0 as i128;
    let mut ids: std::collections::BTreeSet<&String> = give.1.keys().collect();
    ids.extend(get.1.keys());
    let tokens_net: String = ids
        .iter()
        .map(|id| {
            (
                id,
                *get.1.get(*id).unwrap_or(&0) as i128 - *give.1.get(*id).unwrap_or(&0) as i128,
            )
        })
        .filter(|x| x.1 != 0)
        .map(|(id, d)| format!(", {d:+} of token {id}"))
        .collect();
    say!("net for you: {net_erg:+} nanoERG{tokens_net}");
    if !args.iter().any(|a| a == "--sign") {
        say!("not signed (add --sign to sign and send)");
        return Ok(());
    }
    if node.is_empty() {
        return Err("--sign needs --node to send the result".into());
    }
    let Some(p) = phrase.as_deref() else {
        return Err(no_phrase.into());
    };
    let secrets = secrets(p, n_addr);
    if secrets.is_empty() {
        return Err("derivation".into());
    }
    // a wallet under a spending policy holds a request to it like any payment: what it gives, less what it gets back
    let ask = policy::Ask {
        nano: give.0.saturating_sub(get.0),
        tokens: give.1.clone(),
        payees: paid_to.clone(),
        purpose: arg(args, "--purpose", String::new())?,
        what: "request".into(),
    };
    if let Err(e) = policy::reserve(state, &ask, false) {
        return Err(format!("the spending policy does not allow it: {e}").into());
    }
    let signed = match Wallet::from_secrets(secrets).sign_reduced_transaction(reduced, None) {
        Ok(t) => t,
        Err(e) => return Err(format!("sign: {e:?}").into()),
    };
    let json = serde_json::to_string(&signed).unwrap_or_default();
    crate::out::set("signed", true);
    crate::out::set("tx", String::from(signed.id()));
    // The payment is written down as waiting before any node is sent it: if this stops between the node taking it and
    // the file being written, the wallet must not be left thinking those coins are free to spend again.
    let before = w.clone();
    let mut expect: Vec<String> = w.expect.clone();
    expect.extend(
        signed
            .outputs
            .iter()
            .filter(|o| my_trees.contains(&o.ergo_tree))
            .map(|o| String::from(o.box_id())),
    );
    for b in &mine {
        w.boxes.remove(&String::from(b.box_id()));
    }
    w.expect = expect;
    note_pending(&mut w, &signed, &json);
    c.save(&w)?;
    let unsure = match send_tx(node, &json) {
        Ok((n, said)) => { crate::out::set("sent_via", &n); say!("signed and sent via {n}: accepted, {said}"); None }
        // a refusal, or no node reached at all: nothing went out, and the wallet is put back as it was
        Err(NotSent::Refused(c2, body)) => { c.save(&before)?; return Err(format!("the node refused it ({c2}): {}", one_line(body.lines().find(|l| l.contains("detail")).unwrap_or(""))).into()) }
        Err(NotSent::Unreachable(e)) => { c.save(&before)?; return Err(format!("could not send: {e}").into()) }
        Err(NotSent::Unknown(e)) => Some(format!("the node did not answer ({e}). The payment may have gone out: it is kept as waiting, its coins are not counted, and the next sync settles it (sent again if it did not arrive; back in the wallet if it is not in a block after ten). Do not pay again until then")),
    };
    if unsure.is_some() {
        crate::out::set("outcome_unknown", true);
    }
    policy::note(state, &String::from(signed.id()), "");
    match unsure {
        Some(e) => Err(e.into()),
        None => Ok(()),
    }
}
