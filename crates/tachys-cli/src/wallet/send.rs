//! `wallet send`, `consolidate`, `burn`, `babel-offer`, `babel-list`: what is asked for is read, the transaction that
//! does it is built, what it takes out of the wallet is held to the spending policy, and only then is it signed and handed on.

use super::*;

/// What a payment is to do, read from the command line, and what the wallet has to do it with: all that the builders
/// below work from. Reading it asks no node anything.
pub(super) struct Wanted<'a> {
    c: &'a Ctx<'a>,
    /// the wallet's boxes confirmed under a tip the peers agreed on: the only ones a payment spends
    inputs: Vec<ErgoBox>,
    addr: Address,
    my_tree: ErgoTree,
    /// to the first payee (or, for an offer, into it); `more` are the further payees of `--pay`
    amount: u64,
    fee: u64,
    /// the payees and the fee together
    need_total: u64,
    v_out: BoxValue,
    v_fee: BoxValue,
    v_need: BoxValue,
    height: u32,
    token_hex: String,
    token_amount: u64,
    mint_name: String,
    mint_decimals: usize,
    payee_tree: ErgoTree,
    more: Vec<(ErgoTree, u64)>,
    pay_token: Option<Token>,
    /// every token the first payee is paid: `--token` and each `--also-token`
    want_tokens: Vec<Token>,
    /// the token the fee is paid in (`--babel`), and the most of it the fee may cost
    babel_token: String,
    babel_max: u64,
}

/// What a builder made of it: the boxes spent and the boxes made, and what the rest of `cmd_send` must know of them.
pub(super) struct Built {
    selection: BoxSelection<ErgoBox>,
    outs: Vec<ErgoBoxCandidate>,
    /// the scripts paid as payees: boxes under them are the receipt
    payee_trees: Vec<ErgoTree>,
    /// scripts paid that are neither a payee's, ours nor the fee's (a Babel offer)
    other_trees: Vec<ErgoTree>,
    burn: Vec<Token>,
    /// what a Babel offer spent is told: where its recreated box sits among the outputs
    extension: Option<(BoxId, ContextExtension)>,
}

/// Reads what the command line asks for and gathers the boxes that may be spent. What cannot be a payment is refused
/// here, before anything is built.
pub(super) fn wanted<'a>(
    c: &'a Ctx<'a>,
    w: &WalletFile,
    addr: &Address,
    my_tree: &ErgoTree,
) -> Result<Wanted<'a>, Fail> {
    let Ctx {
        args, chain, enc, ..
    } = c;
    let sub = c.sub;
    let token_hex: String = arg(args, "--token", String::new())?;
    let token_amount: u64 = arg(args, "--token-amount", 0)?;
    let mint_name: String = arg(args, "--mint", String::new())?;
    let mint_decimals: usize = arg(args, "--decimals", 0)?;
    // minting pays the new token to ourselves; a token payment carries the minimum ERG a box needs
    let to_self = !mint_name.is_empty() || sub != "send";
    let to: String = if to_self {
        enc.address_to_str(addr)
    } else {
        arg(args, "--to", String::new())?
    };
    // `--pay` and `--also-token` are read below as the word after each; one given last has none, and is misuse, as an
    // option without its value is everywhere (`util::try_arg`), not a payee or token quietly left out
    if let Some(last) = args
        .last()
        .filter(|a| *a == "--pay" || *a == "--also-token")
    {
        return Err(crate::out::Fail::new(2, format!("{last} needs a value")));
    }
    // further payees in the same payment, one fee for all: --pay ADDRESS:NANOERG (repeatable)
    let more: Vec<(ergotree_ir::ergo_tree::ErgoTree, u64)> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--pay")
        .filter_map(|(i, _)| args.get(i + 1))
        .map(|spec| {
            let (a, n) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
            match (
                enc.parse_address_from_str(a)
                    .ok()
                    .and_then(|x| x.script().ok()),
                n.parse::<u64>(),
            ) {
                (Some(t), Ok(v)) if v >= 1_000_000 => Ok((t, v)),
                _ => Err(format!(
                    "--pay wants ADDRESS:NANOERG with at least 1000000 nanoERG, got {spec}"
                )),
            }
        })
        .collect::<Result<_, String>>()?;
    let amount: u64 = {
        let a: u64 = arg(args, "--nanoerg", 0)?;
        if a == 0 && (!token_hex.is_empty() || !mint_name.is_empty() || sub != "send") {
            1_000_000
        } else {
            a
        }
    };
    let fee: u64 = arg(args, "--fee", 1_100_000)?;
    if chain.len() < 10 {
        return Err("needs the ten agreed headers; run `tachys tips` first".into());
    }
    let Ok(payee) = enc.parse_address_from_str(&to) else {
        return Err(format!("--to is not a {} address", net().name).into());
    };
    let Ok(payee_tree) = payee.script() else {
        return Err("payee address has no script".into());
    };
    // Only a box confirmed under a tip the peers agreed on is spent. One proven only in a block a single node has
    // shown is tracked and shown as seen; a payment built on it would be void if that block is not the chain's.
    // A box's stored bytes are used only if they hash to the id they are filed under (as `cmd_request` holds them): the
    // bytes of another box would be signed for in its place, and the balance and the payment would disagree.
    let mut inputs: Vec<ErgoBox> = vec![];
    for (id, b) in w.boxes.iter().filter(|(_, b)| b.confirmed_at.is_some()) {
        let Some(x) = unhex(&b.bytes).and_then(|x| ErgoBox::sigma_parse_bytes(&x).ok()) else {
            return Err(
                "some tracked boxes have no stored bytes; run `tachys wallet status` first".into(),
            );
        };
        if String::from(x.box_id()) != *id {
            return Err(format!("the bytes stored for box {id} are another box's ({}); nothing was signed. Run `tachys wallet status`, which reads each box again from its proof", String::from(x.box_id())).into());
        }
        inputs.push(x);
    }
    if inputs.is_empty() && !w.boxes.is_empty() {
        return Err("none of this wallet's boxes is confirmed under a tip the peers agree on yet; run `tachys wallet status` again after a block".into());
    }
    let more_total: u64 = more
        .iter()
        .try_fold(0u64, |a, m| a.checked_add(m.1))
        .ok_or("the amounts add up to more than can exist")?;
    let need_total = amount
        .checked_add(more_total)
        .and_then(|x| x.checked_add(fee))
        .ok_or("the amounts add up to more than can exist")?;
    let (Ok(v_out), Ok(v_fee), Ok(v_need)) = (
        BoxValue::try_from(amount),
        BoxValue::try_from(fee),
        BoxValue::try_from(need_total),
    ) else {
        return Err("amount or fee out of range".into());
    };
    let height = chain[0].height;
    let pay_token: Option<Token> = if token_hex.is_empty() || sub.starts_with("babel-") {
        None
    } else {
        let (Some(id), Ok(amt)) = (
            ergo_chain_types::Digest32::try_from(token_hex.clone())
                .ok()
                .map(TokenId::from),
            TokenAmount::try_from(token_amount),
        ) else {
            return Err("--token needs a 64-hex id and --token-amount above zero".into());
        };
        Some(Token {
            token_id: id,
            amount: amt,
        })
    };
    // further tokens to the same payee in the same payment: --also-token ID:RAW (repeatable)
    let also: Vec<Token> = if sub != "send" {
        vec![]
    } else {
        args.iter()
            .enumerate()
            .filter(|(_, a)| *a == "--also-token")
            .filter_map(|(i, _)| args.get(i + 1))
            .map(|spec| {
                let (id, n) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
                match (
                    ergo_chain_types::Digest32::try_from(id.to_string())
                        .ok()
                        .map(TokenId::from),
                    n.parse::<u64>()
                        .ok()
                        .and_then(|v| TokenAmount::try_from(v).ok()),
                ) {
                    (Some(id), Some(amount)) => Ok(Token {
                        token_id: id,
                        amount,
                    }),
                    _ => Err(format!(
                        "--also-token wants ID:RAW with an amount above zero, got {spec}"
                    )),
                }
            })
            .collect::<Result<_, String>>()?
    };
    let want_tokens: Vec<Token> = pay_token.iter().cloned().chain(also).collect();
    {
        let mut seen = std::collections::BTreeSet::new();
        if !want_tokens
            .iter()
            .all(|t| seen.insert(String::from(t.token_id)))
        {
            return Err("the same token is named twice".into());
        }
    }
    let babel_token: String = arg(args, "--babel", String::new())?;
    // the most token units the fee may cost: what the person was shown, so that is the most that is signed
    let babel_max: u64 = arg(args, "--babel-max", 0)?;
    Ok(Wanted {
        c,
        inputs,
        addr: addr.clone(),
        my_tree: my_tree.clone(),
        amount,
        fee,
        need_total,
        v_out,
        v_fee,
        v_need,
        height,
        token_hex,
        token_amount,
        mint_name,
        mint_decimals,
        payee_tree,
        more,
        pay_token,
        want_tokens,
        babel_token,
        babel_max,
    })
}

// Fees in a token (EIP-31, "Babel fees"). Someone locks ERG in a box under a public contract with a price;
// anyone may take ERG from it by putting at least (ERG taken / price) of the token back into the same box.
pub(super) fn babel_tree(token: &str) -> Option<ErgoTree> {
    unhex(&babel_hex(token)).and_then(|b| ErgoTree::sigma_parse_bytes(&b).ok())
}
/// The price an offer asks: nanoERG for one unit of the token.
pub(super) fn babel_price(b: &ErgoBox) -> i64 {
    b.additional_registers
        .get_constant(R5)
        .ok()
        .flatten()
        .and_then(|c| c.try_extract_into::<i64>().ok())
        .unwrap_or(0)
}
// The node lists the offers; each one is then proven unspent under the agreed tip (a proof only above it, against
// a block one node has shown, is not used) and checked to sit under the Babel contract for this token, so a node
// can hide an offer but cannot invent one. An error is a lookup that could not be made or could not be checked (the
// node did not answer, or its proof is against a block the agreed chain does not have yet): that is not "no offers",
// and a caller must be able to tell the two apart to know that asking again may find one.
pub(super) fn proven_babel_offers(c: &Ctx, token: &str) -> Result<Vec<ErgoBox>, String> {
    let Ctx {
        node, chain, state, ..
    } = c;
    let price_of = babel_price;
    let Some(tree) = babel_tree(token) else {
        return Ok(vec![]);
    };
    let n = crate::nodes::each(node)
        .into_iter()
        .find(|n| !n.starts_with("p2p://"))
        .unwrap_or_default();
    let Ok(r) = http()
        .post(&format!(
            "{n}/blockchain/box/unspent/byErgoTree?offset=0&limit=50"
        ))
        .set("Content-Type", "application/json")
        .send_string(&format!("\"{}\"", babel_hex(token)))
    else {
        return Err("no node answered when asked for the offers".into());
    };
    let ids: Vec<String> =
        serde_json::from_str::<serde_json::Value>(&r.into_string().unwrap_or_default())
            .ok()
            .and_then(|v| {
                v.as_array().map(|x| {
                    x.iter()
                        .filter_map(|b| b["boxId"].as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut cost = RestCost::default();
    match prove_any_agreed(node, chain, agreed_height(state), &ids, &mut cost) {
        Ok((_, found)) => Ok(found
            .into_iter()
            .flatten()
            .filter_map(|b| ErgoBox::sigma_parse_bytes(&b).ok())
            .filter(|b| b.ergo_tree == tree && price_of(b) > 0)
            .collect()),
        Err(e) => Err(e),
    }
}

/// wallet babel-list: the offers a node lists for a token, each proven. `proven` says the lookup was checked against
/// the agreed tip; when it could not be (exit 1), the empty list means "not known", not "none".
pub(super) fn babel_list(s: &Wanted) -> Ended {
    let (token_hex, node, price_of) = (&s.token_hex, &s.c.node, babel_price);
    let babel_offers = |token: &str| proven_babel_offers(s.c, token);
    if token_hex.len() != 64 || node.is_empty() {
        return Err("babel-list needs --token ID and --node".into());
    }
    crate::out::set("offers", serde_json::json!([]));
    let offers = match babel_offers(token_hex) {
        Ok(found) => {
            crate::out::set("proven", serde_json::json!(true));
            found
        }
        Err(e) => {
            crate::out::set("proven", serde_json::json!(false));
            return Err(format!("the offers could not be checked: {e}").into());
        }
    };
    if offers.is_empty() {
        say!("no proven offers for that token");
    }
    for b in &offers {
        crate::out::push(
            "offers",
            serde_json::json!({ "box": String::from(b.box_id()), "available_nanoerg": b.value.as_u64().saturating_sub(1_000_000), "nanoerg_per_unit": price_of(b) }),
        );
        say!(
            "offer {}  {} nanoERG available at {} nanoERG per token unit",
            String::from(b.box_id()),
            b.value.as_u64().saturating_sub(1_000_000),
            price_of(b)
        );
    }
    Ok(())
}

/// wallet babel-offer: ERG put up for others to take in exchange for a token.
pub(super) fn offer_built(s: &Wanted) -> Result<Built, Fail> {
    let (args, token_hex, addr, inputs) = (s.c.args, &s.token_hex, &s.addr, &s.inputs);
    let (amount, v_out, v_need, height) = (s.amount, s.v_out, s.v_need, s.height);
    let mut other_trees = vec![];
    // put ERG up for others to take in exchange for a token, at our price (nanoERG per smallest token unit)
    let price: i64 = arg(args, "--price", 0)?;
    let Some(tree) = babel_tree(token_hex) else {
        return Err("babel-offer needs --token ID".into());
    };
    let Address::P2Pk(pk) = addr.clone() else {
        return Err("wallet address is not a plain key".into());
    };
    if price <= 0 || amount < 2_000_000 {
        return Err(
            "babel-offer needs --price above zero and --nanoerg of at least 2000000".into(),
        );
    }
    let selection = match SimpleBoxSelector::new().select(inputs.clone(), v_need, &[]) {
        Ok(s) => s,
        Err(e) => return Err(format!("box selection: {e:?}").into()),
    };
    let mut ob = ErgoBoxCandidateBuilder::new(v_out, tree.clone(), height);
    ob.set_register_value(R4, Constant::from(pk));
    ob.set_register_value(R5, Constant::from(price));
    other_trees.push(tree);
    say!(
        "offering {} nanoERG for token {} at {} nanoERG per token unit",
        amount,
        token_hex,
        price
    );
    let (selection, outs) = (
        selection,
        vec![match ob.build() {
            Ok(o) => o,
            Err(e) => return Err(format!("output: {e:?}").into()),
        }],
    );
    Ok(Built {
        selection,
        outs,
        payee_trees: vec![s.payee_tree.clone()],
        other_trees,
        burn: vec![],
        extension: None,
    })
}

/// wallet send --babel: a payment whose fee is paid in a token, with ERG taken from someone's offer.
pub(super) fn babel_paid_built(s: &Wanted) -> Result<Built, Fail> {
    let (node, inputs, want_tokens, more, babel_token, payee_tree) = (
        &s.c.node,
        &s.inputs,
        s.want_tokens.clone(),
        s.more.clone(),
        s.babel_token.clone(),
        &s.payee_tree,
    );
    let (need_total, babel_max, v_out, height) = (s.need_total, s.babel_max, s.v_out, s.height);
    let (mut payee_trees, mut other_trees) = (vec![payee_tree.clone()], vec![]);
    let price_of = babel_price;
    let babel_offers = |token: &str| proven_babel_offers(s.c, token);
    let extension;
    let Some(tree) = babel_tree(&babel_token) else {
        return Err("--babel wants the 64-hex id of the token to pay the fee in".into());
    };
    if node.is_empty() {
        return Err("--babel needs --node to find offers".into());
    }
    if babel_max == 0 {
        return Err("--babel needs --babel-max RAW: the most token units the fee may cost. `wallet babel-list` shows the offers and their prices".into());
    }
    // every box of ours goes in; the change box holds whatever is left
    let mine: Vec<ErgoBox> = inputs.iter().take(40).cloned().collect();
    let my_erg: u64 = mine.iter().map(|b| *b.value.as_u64()).sum();
    let mut left: std::collections::BTreeMap<String, u64> = Default::default();
    for t in mine
        .iter()
        .flat_map(|b| b.tokens.iter().flat_map(|ts| ts.iter()))
    {
        *left.entry(String::from(t.token_id)).or_default() += *t.amount.as_u64();
    }
    // what the payment needs in ERG: the payees, the fee, and the least a change box may hold
    let need = need_total
        .checked_add(1_000_000)
        .ok_or("the amounts add up to more than can exist")?;
    if my_erg >= need {
        return Err("this wallet has enough ERG for the fee; send without --babel".into());
    }
    let short = need - my_erg;
    for t in &want_tokens {
        let have = left.entry(String::from(t.token_id)).or_default();
        if *have < *t.amount.as_u64() {
            return Err("not enough of a token being sent".into());
        }
        *have -= *t.amount.as_u64();
    }
    // the best offer: the one that asks the fewest tokens and can still hold a box's minimum afterwards
    let offers = babel_offers(&babel_token)
        .map_err(|e| format!("the offers for the fee could not be checked: {e}"))?;
    let have_fee_token = *left.get(&babel_token).unwrap_or(&0);
    let cost_in = |b: &ErgoBox| -> u64 {
        let p = price_of(b) as u64;
        short.div_ceil(p)
    };
    let Some(offer) = offers
        .iter()
        .filter(|b| *b.value.as_u64() >= short + 1_000_000 && cost_in(b) <= have_fee_token)
        .min_by_key(|b| cost_in(b))
    else {
        // which of the two it is: no offer holds enough ERG, or the wallet has too few of the token left after what it is sending
        let cheapest = offers
            .iter()
            .filter(|b| *b.value.as_u64() >= short + 1_000_000)
            .map(&cost_in)
            .min();
        return Err(match cheapest {
            Some(cost) => format!("the fee would cost {cost} units of the token and {have_fee_token} are left after what is being sent: send {} fewer, or pay the fee in ERG", cost - have_fee_token),
            None => format!("no proven offer holds the {short} nanoERG this payment is short of ({} offers seen)", offers.len()),
        }.into());
    };
    let paid = cost_in(offer);
    // The node lists the offers, so it can leave the good ones out. Nothing is signed above what was agreed to.
    if paid > babel_max {
        return Err(format!("the fee would cost {paid} token units at the best offer the node lists now, more than the {babel_max} allowed; nothing was signed. Look the price up again").into());
    }
    *left.entry(babel_token.clone()).or_default() -= paid;
    crate::out::set("fee_token", &babel_token);
    crate::out::set("fee_token_units", paid);
    say!("fee in tokens: taking {short} nanoERG from offer {} at {} nanoERG per unit, paying {paid} token units", String::from(offer.box_id()), price_of(offer));
    // the offer box, recreated with less ERG and more tokens; the contract checks this output
    let Ok(v_back) = BoxValue::try_from(*offer.value.as_u64() - short) else {
        return Err("offer value".into());
    };
    let mut bb = ErgoBoxCandidateBuilder::new(v_back, tree.clone(), height);
    let held: Vec<Token> = offer
        .tokens
        .iter()
        .flat_map(|ts| ts.iter().cloned())
        .collect();
    let before = held.first().map(|t| *t.amount.as_u64()).unwrap_or(0);
    let (Some(fee_id), Ok(amt)) = (
        ergo_chain_types::Digest32::try_from(babel_token.clone())
            .ok()
            .map(TokenId::from),
        TokenAmount::try_from(before + paid),
    ) else {
        return Err("fee token".into());
    };
    bb.add_token(Token {
        token_id: fee_id,
        amount: amt,
    });
    for t in held.iter().skip(1) {
        bb.add_token(*t);
    }
    let (Ok(Some(r4)), Ok(Some(r5))) = (
        offer.additional_registers.get_constant(R4),
        offer.additional_registers.get_constant(R5),
    ) else {
        return Err("offer registers".into());
    };
    let id_bytes: Vec<u8> = ergo_chain_types::Digest32::from(offer.box_id()).0.to_vec();
    bb.set_register_value(R4, r4);
    bb.set_register_value(R5, r5);
    bb.set_register_value(R6, Constant::from(id_bytes));
    let mut ob = ErgoBoxCandidateBuilder::new(v_out, payee_tree.clone(), height);
    for t in &want_tokens {
        ob.add_token(*t);
    }
    let mut outs = vec![match ob.build() {
        Ok(o) => o,
        Err(e) => return Err(format!("output: {e:?}").into()),
    }];
    for (t, value) in &more {
        let Ok(v) = BoxValue::try_from(*value) else {
            return Err("--pay amount out of range".into());
        };
        outs.push(
            match ErgoBoxCandidateBuilder::new(v, t.clone(), height).build() {
                Ok(o) => o,
                Err(e) => return Err(format!("output: {e:?}").into()),
            },
        );
        payee_trees.push(t.clone());
    }
    outs.push(match bb.build() {
        Ok(o) => o,
        Err(e) => return Err(format!("offer output: {e:?}").into()),
    });
    // the contract is told where its recreated box sits among the outputs
    let mut ext = ergotree_ir::chain::context_extension::ContextExtension::empty();
    ext.values
        .insert(0, Constant::from((outs.len() - 1) as i32));
    extension = Some((offer.box_id(), ext));
    other_trees.push(tree);
    let change_tokens: Vec<Token> = left
        .iter()
        .filter(|(_, a)| **a > 0)
        .filter_map(|(id, a)| {
            Some(Token {
                token_id: TokenId::from(ergo_chain_types::Digest32::try_from(id.clone()).ok()?),
                amount: TokenAmount::try_from(*a).ok()?,
            })
        })
        .collect();
    let Ok(v_change) = BoxValue::try_from(1_000_000u64) else {
        return Err("value".into());
    };
    let change = ErgoBoxAssetsData {
        value: v_change,
        tokens: BoxTokens::from_vec(change_tokens).ok(),
    };
    let mut all = mine.clone();
    all.push(offer.clone());
    let Ok(bx) = all.try_into() else {
        return Err("box list".into());
    };
    let (selection, outs) = (
        BoxSelection {
            boxes: bx,
            change_boxes: vec![change],
        },
        outs,
    );
    Ok(Built {
        selection,
        outs,
        payee_trees,
        other_trees,
        burn: vec![],
        extension,
    })
}

/// wallet consolidate: every tracked box into one, back to ourselves.
pub(super) fn consolidation_built(s: &Wanted) -> Result<Built, Fail> {
    let (inputs, my_tree, fee, height) = (&s.inputs, &s.my_tree, s.fee, s.height);
    // every tracked box (up to 60) into one box back to ourselves: fewer boxes means smaller proofs later
    let boxes: Vec<ErgoBox> = inputs.iter().take(60).cloned().collect();
    if boxes.len() < 2 {
        return Err("nothing to consolidate: fewer than two boxes".into());
    }
    let total: u64 = boxes.iter().map(|b| *b.value.as_u64()).sum();
    let mut sums: Vec<(TokenId, u64)> = vec![];
    for t in boxes
        .iter()
        .flat_map(|b| b.tokens.iter().flat_map(|ts| ts.iter()))
    {
        match sums.iter_mut().find(|x| x.0 == t.token_id) {
            Some(x) => x.1 += *t.amount.as_u64(),
            None => sums.push((t.token_id, *t.amount.as_u64())),
        }
    }
    let Ok(v) = BoxValue::try_from(total.saturating_sub(fee)) else {
        return Err("not enough ERG to pay the fee".into());
    };
    let mut ob = ErgoBoxCandidateBuilder::new(v, my_tree.clone(), height);
    for (id, amt) in &sums {
        if let Ok(a) = TokenAmount::try_from(*amt) {
            ob.add_token(Token {
                token_id: *id,
                amount: a,
            });
        }
    }
    let out = match ob.build() {
        Ok(o) => o,
        Err(e) => return Err(format!("output: {e:?}").into()),
    };
    let Ok(bx) = boxes.clone().try_into() else {
        return Err("box list".into());
    };
    say!(
        "consolidating {} boxes ({} token kinds) into one",
        boxes.len(),
        sums.len()
    );
    let (selection, outs) = (
        BoxSelection {
            boxes: bx,
            change_boxes: vec![],
        },
        vec![out],
    );
    Ok(Built {
        selection,
        outs,
        payee_trees: vec![s.payee_tree.clone()],
        other_trees: vec![],
        burn: vec![],
        extension: None,
    })
}

/// wallet burn: some of a token destroyed.
pub(super) fn burn_built(s: &Wanted) -> Result<Built, Fail> {
    let (inputs, my_tree, token_hex, pay_token) =
        (&s.inputs, &s.my_tree, &s.token_hex, s.pay_token);
    let (fee, height, token_amount) = (s.fee, s.height, s.token_amount);
    let mut burn = vec![];
    // destroy some of a token: it is selected as an input and appears in no output, which needs an explicit permit
    let Some(t) = pay_token else {
        return Err("burn needs --token ID and --token-amount RAW".into());
    };
    let Ok(need) = BoxValue::try_from(1_000_000 + fee) else {
        return Err("fee out of range".into());
    };
    let sel = match SimpleBoxSelector::new().select(inputs.clone(), need, &[t]) {
        Ok(x) => x,
        Err(e) => return Err(format!("box selection: {e:?}").into()),
    };
    let Ok(v) = BoxValue::try_from(1_000_000u64) else {
        return Err("value".into());
    };
    let out = match ErgoBoxCandidateBuilder::new(v, my_tree.clone(), height).build() {
        Ok(o) => o,
        Err(e) => return Err(format!("output: {e:?}").into()),
    };
    say!("burning {} units of token {}", token_amount, token_hex);
    burn.push(t);
    let (selection, outs) = (sel, vec![out]);
    Ok(Built {
        selection,
        outs,
        payee_trees: vec![s.payee_tree.clone()],
        other_trees: vec![],
        burn,
        extension: None,
    })
}

/// wallet send: a payment to one payee or several, of ERG and tokens; or a new token minted to ourselves.
pub(super) fn payment_built(s: &Wanted) -> Result<Built, Fail> {
    let (inputs, want_tokens, more, payee_tree, mint_name) = (
        &s.inputs,
        s.want_tokens.clone(),
        s.more.clone(),
        &s.payee_tree,
        &s.mint_name,
    );
    let (v_need, v_out, height, token_amount, mint_decimals) =
        (s.v_need, s.v_out, s.height, s.token_amount, s.mint_decimals);
    let mut payee_trees = vec![payee_tree.clone()];
    let selection = match SimpleBoxSelector::new().select(inputs.clone(), v_need, &want_tokens) {
        Ok(s) => s,
        Err(e) => return Err(format!("box selection: {e:?}").into()),
    };
    let mut ob = ErgoBoxCandidateBuilder::new(v_out, payee_tree.clone(), height);
    for t in &want_tokens {
        ob.add_token(*t);
    }
    if !mint_name.is_empty() {
        // a new token's id is the id of the first box the transaction spends; its name and decimals go in the box's registers
        let Ok(amt) = TokenAmount::try_from(token_amount) else {
            return Err("--mint needs --token-amount above zero".into());
        };
        let id = TokenId::from(selection.boxes.first().box_id());
        ob.mint_token(
            Token {
                token_id: id,
                amount: amt,
            },
            mint_name.clone(),
            String::new(),
            mint_decimals,
        );
        say!(
            "minting {} units of '{}' ({} decimals), token id {}",
            token_amount,
            mint_name,
            mint_decimals,
            String::from(id)
        );
    }
    let mut outs = vec![match ob.build() {
        Ok(o) => o,
        Err(e) => return Err(format!("output: {e:?}").into()),
    }];
    for (tree, value) in &more {
        let Ok(v) = BoxValue::try_from(*value) else {
            return Err("--pay amount out of range".into());
        };
        outs.push(
            match ErgoBoxCandidateBuilder::new(v, tree.clone(), height).build() {
                Ok(o) => o,
                Err(e) => return Err(format!("output: {e:?}").into()),
            },
        );
        payee_trees.push(tree.clone());
    }
    let (selection, outs) = (selection, outs);
    Ok(Built {
        selection,
        outs,
        payee_trees,
        other_trees: vec![],
        burn: vec![],
        extension: None,
    })
}

/// A payment given up as "not included" is still a valid transaction: a node may yet hold it, and so may a payee
/// handed it offline. The coins it spends count as this wallet's again, so the next payment could be made from
/// other boxes and both could confirm. So the next payment always spends one of those coins: then only one of the
/// two can ever be in the chain. (Its value comes straight back as change.)
pub(super) fn with_a_released_coin(
    selection: BoxSelection<ErgoBox>,
    inputs: &[ErgoBox],
    done: &[Done],
) -> Result<BoxSelection<ErgoBox>, Fail> {
    let released: Vec<String> = done
        .iter()
        .filter(|d| d.result == "not included")
        .flat_map(|d| d.inputs.iter().cloned())
        .collect();
    let is_released = |b: &ErgoBox| released.contains(&String::from(b.box_id()));
    Ok(match inputs.iter().find(|b| is_released(b)) {
        Some(extra) if !selection.boxes.iter().any(is_released) => {
            let mut boxes = selection.boxes.as_vec().clone();
            boxes.push(extra.clone());
            let mut change = selection.change_boxes.clone();
            change.push(ErgoBoxAssetsData {
                value: extra.value,
                tokens: extra.tokens.clone(),
            });
            BoxSelection {
                boxes: boxes.try_into().map_err(|_| "box list".to_string())?,
                change_boxes: change,
            }
        }
        _ => selection,
    })
}

/// The library pays the fee to mainnet's fee script. A network with another reward delay has
/// another script; swap it in on the one output that is neither the payment nor our change.
pub(super) fn with_fee_script(
    unsigned: UnsignedTransaction,
    payee_trees: &[ErgoTree],
    my_trees: &[ErgoTree],
    other_trees: &[ErgoTree],
) -> Result<UnsignedTransaction, Fail> {
    if net().fee_tree.is_empty() {
        return Ok(unsigned);
    }
    let Some(ft) = unhex(net().fee_tree).and_then(|b| ErgoTree::sigma_parse_bytes(&b).ok()) else {
        return Err("net file fee_tree is not a script".into());
    };
    let outs: Vec<_> = unsigned
        .output_candidates
        .iter()
        .cloned()
        .map(|mut c| {
            if !payee_trees.contains(&c.ergo_tree)
                && !my_trees.contains(&c.ergo_tree)
                && !other_trees.contains(&c.ergo_tree)
            {
                c.ergo_tree = ft.clone();
            }
            c
        })
        .collect();
    match UnsignedTransaction::new_from_vec(
        unsigned.inputs.as_vec().clone(),
        unsigned
            .data_inputs
            .as_ref()
            .map(|d| d.as_vec().clone())
            .unwrap_or_default(),
        outs,
    ) {
        Ok(t) => Ok(t),
        Err(e) => Err(format!("fee script swap: {e:?}").into()),
    }
}

/// What this transaction takes out of the wallet, read off the transaction itself: what its own boxes put in,
/// less what comes back to its own addresses. A spending policy, if the wallet has one, is applied to this.
/// Handed back: the nanoERG that leaves (the network fee is in it), the token units that leave by token id, and the
/// address of every output that is neither the wallet's own nor a fee. Nothing is read but the transaction.
pub(super) fn leaves_wallet(
    spent: &[ErgoBox],
    outputs: &[ErgoBoxCandidate],
    my_trees: &[ErgoTree],
    extension: Option<&(BoxId, ContextExtension)>,
    enc: &AddressEncoder,
) -> (u64, std::collections::BTreeMap<String, u64>, Vec<String>) {
    let fee_tree = fee_tree_hex();
    let (mut nano_in, mut nano_back) = (0u64, 0u64);
    let (mut tok_in, mut tok_back): (
        std::collections::BTreeMap<String, u64>,
        std::collections::BTreeMap<String, u64>,
    ) = Default::default();
    for b in spent.iter().filter(|b| my_trees.contains(&b.ergo_tree)) {
        nano_in = nano_in.saturating_add(*b.value.as_u64());
        for t in b.tokens.iter().flat_map(|ts| ts.iter()) {
            let n = tok_in.entry(String::from(t.token_id)).or_default();
            *n = n.saturating_add(*t.amount.as_u64());
        }
    }
    // A Babel fee recreates the offer box it takes ERG from, under the offer's own script: the fee's counterpart, as
    // the miner's fee output is, not a payee the owner chose. Only that box: the output the contract is told it
    // is, under the script of the offer spent, with no more ERG than the offer had (it takes none of ours). Any
    // other output to that script is a payee. The tokens it is paid still count as leaving the wallet.
    let offer_back: Option<(usize, &ErgoBox)> = extension.and_then(|(id, ext)| {
        let at = ext.values.get(&0)?.clone().try_extract_into::<i32>().ok()?;
        Some((
            usize::try_from(at).ok()?,
            spent.iter().find(|b| b.box_id() == *id)?,
        ))
    });
    let mut payees: Vec<String> = vec![];
    for (i, o) in outputs.iter().enumerate() {
        if offer_back.is_some_and(|(at, offer)| {
            at == i && o.ergo_tree == offer.ergo_tree && o.value.as_u64() <= offer.value.as_u64()
        }) {
            continue;
        }
        if my_trees.contains(&o.ergo_tree) {
            nano_back = nano_back.saturating_add(*o.value.as_u64());
            for t in o.tokens.iter().flat_map(|ts| ts.iter()) {
                let n = tok_back.entry(String::from(t.token_id)).or_default();
                *n = n.saturating_add(*t.amount.as_u64());
            }
        } else {
            let tree_hex = o
                .ergo_tree
                .sigma_serialize_bytes()
                .map(|b| hex(&b))
                .unwrap_or_default();
            if tree_hex == fee_tree {
                continue;
            }
            let a = Address::recreate_from_ergo_tree(&o.ergo_tree)
                .map(|a| enc.address_to_str(&a))
                .unwrap_or_else(|_| format!("script {}", clip(&tree_hex, 40)));
            if !payees.contains(&a) {
                payees.push(a);
            }
        }
    }
    let tokens = tok_in
        .into_iter()
        .filter_map(|(id, n)| {
            let left = n.saturating_sub(*tok_back.get(&id).unwrap_or(&0));
            (left > 0).then_some((id, left))
        })
        .collect();
    (nano_in.saturating_sub(nano_back), tokens, payees)
}

/// A payment signed: say what it is, keep it, and hand it on as asked: written as a parcel for someone to carry
/// (`--parcel-out`), sent to a node (`--broadcast`), or left in the state directory.
pub(super) fn hand_over(
    c: &Ctx,
    mut w: WalletFile,
    signed: Transaction,
    spent: Vec<ErgoBox>,
    payee_trees: &[ErgoTree],
    my_trees: &[ErgoTree],
) -> Ended {
    let Ctx {
        args, state, node, ..
    } = c;
    let json = serde_json::to_string(&signed).unwrap_or_default();
    let bx = |b: &ErgoBox| String::from(b.box_id());
    let payee_ids: Vec<String> = signed
        .outputs
        .iter()
        .filter(|o| payee_trees.contains(&o.ergo_tree))
        .map(bx)
        .collect();
    let change_ids: Vec<String> = signed
        .outputs
        .iter()
        .filter(|o| my_trees.contains(&o.ergo_tree))
        .map(bx)
        .collect();
    crate::out::set("signed", true);
    crate::out::set("tx", String::from(signed.id()));
    crate::out::set("receipt", &payee_ids);
    crate::out::set("change", &change_ids);
    say!(
        "signed: tx {} ({} B as JSON), {} inputs, {} outputs",
        signed.id(),
        json.len(),
        spent.len(),
        signed.outputs.len()
    );
    say!(
        "receipt for the payee (box ids to hand over): {}",
        payee_ids.join(",")
    );
    say!(
        "change we will look for ourselves: {}",
        if change_ids.is_empty() {
            "none".into()
        } else {
            change_ids.join(",")
        }
    );
    policy::note(
        state,
        &String::from(signed.id()),
        payee_ids.first().map(|s| s.as_str()).unwrap_or(""),
    );
    let _ = write_private(&format!("{state}/last-tx.json"), json.as_bytes());
    let parcel_out: String = arg(args, "--parcel-out", String::new())?;
    if !parcel_out.is_empty() {
        // offline: write the payment as a parcel for someone else to carry, and do our own
        // bookkeeping now (inputs are spent as far as we are concerned; change is expected)
        use ergotree_ir::serialization::SigmaSerializable;
        let bytes = match signed.sigma_serialize_bytes() {
            Ok(b) => b,
            Err(e) => return Err(format!("serialize: {e:?}").into()),
        };
        let parcel = serde_json::json!({ "tx_id": String::from(signed.id()), "tx_bytes": hex(&bytes), "tx_json": json });
        if std::fs::write(&parcel_out, parcel.to_string()).is_err() {
            return Err(format!("cannot write {parcel_out}").into());
        }
        let mut expect: Vec<String> = w.expect.clone();
        expect.extend(change_ids.clone());
        for b in &spent {
            w.boxes.remove(&bx(b));
        }
        w.expect = expect;
        note_pending(&mut w, &signed, &json);
        c.save(&w)?;
        crate::out::set("parcel", &parcel_out);
        crate::out::set("tx_bytes", hex(&bytes));
        say!("parcel written to {parcel_out} ({} B of transaction); hand it to any wallet that can reach the network", bytes.len());
        return Ok(());
    }
    if args.iter().any(|a| a == "--broadcast") {
        if node.is_empty() {
            return Err("--broadcast needs --node".into());
        }
        // written down as waiting before any node is sent it (see `cmd_request`): spent inputs go, change is expected
        let before = w.clone();
        let mut expect: Vec<String> = w.expect.clone();
        expect.extend(change_ids);
        for b in &spent {
            w.boxes.remove(&bx(b));
        }
        w.expect = expect;
        note_pending(&mut w, &signed, &json);
        c.save(&w)?;
        let unsure = match send_tx(node, &json) {
            Ok((n, said)) => { crate::out::set("broadcast", serde_json::json!({ "node": n, "accepted": true })); say!("broadcast via {n}: accepted, {said}"); None }
            // a refusal, or no node reached at all: nothing went out, and the wallet is put back as it was
            Err(NotSent::Refused(code, body)) => { c.save(&before)?; return Err(format!("node refused ({code}): {}", one_line(&body)).into()) }
            Err(NotSent::Unreachable(e)) => { c.save(&before)?; return Err(format!("broadcast: {e}").into()) }
            Err(NotSent::Unknown(e)) => Some(format!("the node did not answer ({e}). The payment may have gone out: it is kept as waiting, its coins are not counted, and the next sync settles it (sent again if it did not arrive; back in the wallet if it is not in a block after ten). Do not pay again until then")),
        };
        if unsure.is_some() {
            crate::out::set("outcome_unknown", true);
        }
        if let Some(e) = unsure {
            return Err(e.into());
        }
    } else {
        say!("not broadcast (add --broadcast); signed transaction saved to {state}/last-tx.json");
    }
    Ok(())
}

/// wallet send | consolidate | burn | babel-offer | babel-list: what is asked for is read (`wanted`), the transaction
/// that does it is built (one builder for each), what it takes out of the wallet is read off it and held to the
/// spending policy (`leaves_wallet`), and only then is it signed and handed on (`hand_over`).
pub(super) fn cmd_send(c: &Ctx, o: Open) -> Ended {
    let Ctx {
        args,
        state,
        chain,
        enc,
        ..
    } = c;
    let sub = c.sub;
    let Open {
        w,
        phrase,
        no_phrase,
        locked,
        bad_password,
        addr,
        my_tree,
        n_addr,
        my_trees,
        ..
    } = o;
    let s = wanted(c, &w, &addr, &my_tree)?;
    if sub == "babel-list" {
        return babel_list(&s);
    }
    let Built {
        selection,
        outs,
        payee_trees,
        other_trees,
        burn,
        extension,
    } = match sub {
        "babel-offer" => offer_built(&s)?,
        "send" if !s.babel_token.is_empty() && s.mint_name.is_empty() => babel_paid_built(&s)?,
        "consolidate" => consolidation_built(&s)?,
        "burn" => burn_built(&s)?,
        _ => payment_built(&s)?,
    };
    let selection = with_a_released_coin(selection, &s.inputs, &w.done)?;
    let (height, v_fee, fee) = (s.height, s.v_fee, s.fee);
    let mut tb = TxBuilder::new(selection.clone(), outs, height, v_fee, addr.clone());
    if !burn.is_empty() {
        tb.set_token_burn_permit(burn.clone());
    }
    if let Some((id, ext)) = extension.clone() {
        tb.set_context_extension(id, ext);
    }
    let unsigned = match tb.build() {
        Ok(t) => t,
        Err(e) => return Err(format!("build: {e:?}").into()),
    };
    let unsigned = with_fee_script(unsigned, &payee_trees, &my_trees, &other_trees)?;
    let headers: [Header; 10] = match chain[..10].to_vec().try_into() {
        Ok(h) => h,
        Err(_) => return Err("header context".into()),
    };
    let ctx = ErgoStateContext::new(
        PreHeader::from(headers[0].clone()),
        headers,
        Parameters::default(),
    );
    let spent: Vec<ErgoBox> = selection.boxes.as_vec().clone();
    // a spending policy, if the wallet has one, is applied to this
    let (nano, tokens, payees) = leaves_wallet(
        &spent,
        unsigned.output_candidates.as_vec(),
        &my_trees,
        extension.as_ref(),
        enc,
    );
    let ask = policy::Ask {
        nano,
        tokens,
        payees,
        purpose: arg(args, "--purpose", String::new())?,
        what: sub.to_string(),
    };
    let tc = match TransactionContext::new(unsigned, spent.clone(), vec![]) {
        Ok(t) => t,
        Err(e) => return Err(format!("context: {e:?}").into()),
    };
    if args.iter().any(|a| a == "--request-out") {
        // not signed: written as a signing request (ErgoPay, static form) for a wallet to show, sign and send
        use base64::Engine;
        let reduced = match ergo_lib::chain::transaction::reduced::reduce_tx(tc.clone(), &ctx) {
            Ok(r) => r,
            Err(e) => return Err(format!("reduce: {e:?}").into()),
        };
        let bytes = match reduced.sigma_serialize_bytes() {
            Ok(b) => b,
            Err(e) => return Err(format!("serialize: {e:?}").into()),
        };
        crate::out::set(
            "request",
            format!(
                "ergopay:{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
            ),
        );
        crate::out::set("signed", false);
        say!(
            "ergopay:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
        );
        return Ok(());
    }
    // what the payment does, read off the transaction as built: the same numbers the spending policy is held to
    crate::out::set("leaves_wallet_nanoerg", ask.nano);
    crate::out::set("tokens_out", &ask.tokens);
    crate::out::set("payees", &ask.payees);
    crate::out::set("fee_nanoerg", fee);
    crate::out::set("signed", false);
    if args.iter().any(|a| a == "--dry-run") {
        // a wallet with no phrase at all (read-only) could never sign this, nor one given a wrong password: a dry run that
        // says yes would mislead
        if phrase.is_none() && (!locked || bad_password) {
            return Err(no_phrase.into());
        }
        // built and checked against the policy, as a real payment is; nothing is signed, written or counted
        if let Err(e) = policy::reserve(state, &ask, true) {
            return Err(format!("the spending policy does not allow it: {e}").into());
        }
        say!("dry run: {} nanoERG would leave this wallet (fee {fee} included), to {}; nothing signed", ask.nano, if ask.payees.is_empty() { "itself".to_string() } else { ask.payees.join(", ") });
        return Ok(());
    }
    let Some(p) = phrase.as_deref() else {
        return Err(no_phrase.into());
    };
    let secrets = secrets(p, n_addr);
    if secrets.is_empty() {
        return Err("derivation".into());
    }
    // written to the payments log before the signature is made, so it counts whether or not the payment goes through
    if let Err(e) = policy::reserve(state, &ask, false) {
        return Err(format!("the spending policy does not allow it: {e}").into());
    }
    let signed = match Wallet::from_secrets(secrets).sign_transaction(tc, &ctx, None) {
        Ok(t) => t,
        Err(e) => return Err(format!("sign: {e:?}").into()),
    };
    hand_over(c, w, signed, spent, &payee_trees, &my_trees)
}
