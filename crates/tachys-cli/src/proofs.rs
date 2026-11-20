//! Box proofs: a node's batch proof is checked against the state root of a header on the agreed chain.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ergo_chain_types::Header;
use sigma_ser::ScorexSerializable;

use crate::chain::{carries_on, fresh_enough, FRESH, header_ok, id32, load_chain, now_ms, parent32};
use crate::network::{chain_params, release_anchor};
use crate::util::{arg, clip, http, one_line, unhex};

/// tachys boxes: prove that boxes are unspent at a header of the chain `tachys tips` agreed on.
///   --ids ID[,ID...]   the receipt path: ids handed over by a payer; no index, no address revealed
///   --address ADDR     the reconciliation path: ask an indexed node which boxes the address holds
/// The node is untrusted: its batch proof must reproduce the state root of an agreed header, and
/// every returned box must hash to its id.
pub(crate) fn boxes(args: &[String]) -> crate::out::Ended {
    use ergotree_ir::chain::ergo_box::ErgoBox;
    use ergotree_ir::serialization::SigmaSerializable;

    let state: String = arg(args, "--state", "state".to_string())?;
    let node: String = arg(args, "--node", String::new())?;
    let ids_arg: String = arg(args, "--ids", String::new())?;
    let address: String = arg(args, "--address", String::new())?;
    if node.is_empty() || (ids_arg.is_empty() == address.is_empty()) { return Err(crate::out::Fail::new(2, "usage: tachys boxes --node URL --state DIR (--ids ID[,ID...] | --address ADDR)".to_string())) }
    let node = crate::nodes::resolved(&node, &state)?;

    // the agreed chain from the last `tachys tips`
    let chain = load_chain(&state);
    if chain.is_empty() { return Err(crate::out::Fail::new(2, format!("no agreed chain in {state}; run `tachys tips --state {state}` first"))) }
    let mut cost = RestCost::default();

    // which boxes
    let ids: Vec<String> = if !ids_arg.is_empty() {
        ids_arg.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        cost.sent += address.len(); cost.requests += 1;
        let resp = http().post(&format!("{}/blockchain/box/unspent/byAddress?offset=0&limit=200", crate::nodes::each(&node).first().cloned().unwrap_or_default())).set("Content-Type", "application/json").send_string(&address);
        let text = match resp { Ok(r) => r.into_string().unwrap_or_default(), Err(e) => { say!("discovery failed: {e}"); return Err(crate::out::Fail::said(1)) } };
        cost.received += text.len();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        if !v.is_array() { say!("discovery failed: the node's answer is not a list of boxes"); return Err(crate::out::Fail::said(1)) }
        let ids: Vec<String> = v.as_array().map(|a| a.iter().filter_map(|b| b["boxId"].as_str().map(String::from)).collect()).unwrap_or_default();
        say!("discovery: node lists {} unspent boxes for the address ({} B)", ids.len(), text.len());
        ids
    };
    if ids.is_empty() { say!("no boxes to prove"); return Ok(()) }
    let proven = prove_any(&node, &chain, &ids, &mut cost);
    say!("cost: {} requests, {} B sent, {} B received", cost.requests, cost.sent, cost.received);
    match proven {
        Err(e) => { say!("verdict: NOT PROVEN. {e}"); return Err(crate::out::Fail::said(1)) }
        Ok((height, found)) => {
            let (mut total, mut present, mut tokens) = (0u64, 0usize, 0usize);
            for (id, f) in ids.iter().zip(found.iter()) {
                match f {
                    None => { crate::out::push("checked", serde_json::json!({ "id": id, "state": "absent", "height": height })); say!("  {id}  ABSENT at height {height} (spent or never existed)") }
                    Some(bytes) => match ErgoBox::sigma_parse_bytes(bytes) {
                        Ok(b) => { present += 1; total += *b.value.as_u64(); tokens += b.tokens.as_ref().map_or(0, |t| t.len());
                            crate::out::push("checked", serde_json::json!({ "id": id, "state": "unspent", "height": height, "nanoerg": b.value.as_u64(), "tokens": b.tokens.as_ref().map_or(0, |t| t.len()), "created_at": b.creation_height }));
                            say!("  {id}  {} nanoERG, {}, created at {}", b.value.as_u64(), crate::util::count(b.tokens.as_ref().map_or(0, |t| t.len()), "token"), b.creation_height); }
                        Err(e) => { crate::out::push("checked", serde_json::json!({ "id": id, "state": "undecodable", "height": height })); say!("  {id}  present, {} B, could not decode: {e:?}", bytes.len()) }
                    },
                }
            }
            crate::out::set("proven_at", height); crate::out::set("unspent", present); crate::out::set("nanoerg", total);
            say!("verdict: PROVEN at height {height}: {present} of {} boxes unspent, {:.9} ERG, {}", ids.len(), total as f64 / 1e9, crate::util::count(tokens, "token entry").replace("entrys", "entries"));
        }
    }
    Ok(())
}

/// Read a node's batch proof against one state root: for each key, the box bytes or `None` (proven absent).
/// `None` overall if the proof does not answer for every key, a returned box does not hash to its id, or the proof
/// does not reproduce the root. The proof bytes are a node's: a malformed proof that makes the verifier panic is
/// caught here and counts as no proof. (Where the build aborts on panic, as the phone profile does, the process
/// still ends; there the core runs as a child process, which is what contains it.)
pub(crate) fn lookup_all(root: &[u8], proof: &[u8], keys: &[[u8; 32]]) -> Option<Vec<Option<Vec<u8>>>> {
    use bytes::Bytes;
    use ergo_avltree_rust::authenticated_tree_ops::AuthenticatedTreeOps;
    use ergo_avltree_rust::batch_avl_verifier::BatchAVLVerifier;
    use ergo_avltree_rust::batch_node::{AVLTree, Node, NodeHeader};
    use ergo_avltree_rust::operation::Operation;
    std::panic::catch_unwind(|| {
        let tree = AVLTree::new(|d| Node::LabelOnly(NodeHeader::new(Some(*d), None)), 32, None);
        let mut bv = BatchAVLVerifier::new(&Bytes::copy_from_slice(root), &Bytes::copy_from_slice(proof), tree, None, None).ok()?;
        let mut found = vec![];
        for k in keys {
            match bv.perform_one_operation(&Operation::Lookup(Bytes::copy_from_slice(k))) {
                Ok(Some(v)) if sigma_util::hash::blake2b256_hash(&v).as_ref() == k.as_slice() => found.push(Some(v.to_vec())),
                Ok(None) => found.push(None),
                _ => return None,
            }
        }
        bv.digest().is_some_and(|d| d.as_ref() == root).then_some(found)
    }).unwrap_or(None)
}
fn keys_of(ids: &[String]) -> Result<Vec<[u8; 32]>, String> {
    ids.iter().map(|s| unhex(s).and_then(|b| <[u8; 32]>::try_from(b).ok())).collect::<Option<_>>().ok_or("a box id is not 32 bytes of hex".into())
}

/// What a proof showed: the height it is against and, for each id asked, the box bytes (`None`: proven absent).
pub(crate) type Proven = (u32, Vec<Option<Vec<u8>>>);

/// Network cost of a wallet operation against a REST node.
#[derive(Default)]
pub(crate) struct RestCost { pub(crate) requests: usize, pub(crate) sent: usize, pub(crate) received: usize }

/// Ask `node` for a batch proof of `ids` and check it against the newest headers of the stored chain. Returns the
/// height it proves at and, per id, the box bytes (None = proven absent).
pub(crate) fn prove(node: &str, chain: &[Header], ids: &[String], cost: &mut RestCost) -> Result<Proven, String> { prove_reading(node, chain, None, ids, cost) }

/// `prove`, and if the proof matches none of the newest headers, against the newest `FRESH` at or below the agreed tip
/// (`agreed`, its height). Headers a node added above the agreed tip are kept in the stored chain for the next proof,
/// and enough of them push the agreed tip out of the newest few; an honest node at the agreed tip, or a little behind
/// it, is then still read. What is proven at or below the agreed tip is what may settle; nothing above it does. Each of
/// those headers is held to the age a proof may rest on, as the newest is: added headers keep the chain's head recent
/// while the agreed tip grows old, and a proof against an old tip would show coins spent since as unspent.
pub(crate) fn prove_reading(node: &str, chain: &[Header], agreed: Option<u32>, ids: &[String], cost: &mut RestCost) -> Result<Proven, String> {
    let max_age = release_anchor().max_tip_age_ms;
    fresh_enough(chain, now_ms(), max_age)?;
    let keys = keys_of(ids)?;
    let body = serde_json::to_string(ids).map_err(|e| e.to_string())?;
    cost.requests += 1; cost.sent += body.len();
    let text = http().post(&format!("{node}/utxo/getBoxesBinaryProof")).set("Content-Type", "application/json").send_string(&body)
        .map_err(|e| format!("proof request failed: {e}"))?.into_string().map_err(|e| e.to_string())?;
    cost.received += text.len();
    let proof = serde_json::from_str::<String>(&text).ok().and_then(|h| unhex(&h)).ok_or_else(|| format!("node did not return a proof: {}", clip(&one_line(&text), 120)))?;
    // try the proof against the state roots of the newest stored headers, newest first; then the newest at or below the agreed tip
    let below_agreed = chain.iter().filter(|h| agreed.is_some_and(|a| h.height <= a)).take(FRESH).filter(|h| fresh_enough(std::slice::from_ref(*h), now_ms(), max_age).is_ok());
    let newest: Vec<&Header> = chain.iter().take(FRESH).collect();
    for h in newest.iter().copied().chain(below_agreed.filter(|h| !newest.iter().any(|n| n.id == h.id))) {
        let root: Vec<u8> = h.state_root.into();
        if let Some(found) = lookup_all(&root, &proof, &keys) { return Ok((h.height, found)) }
    }
    Err(format!("the proof matches no header of the agreed chain among its newest {FRESH} ({}..{}); the node may be ahead of it, or well behind: run `tachys tips` again",
        chain.iter().take(FRESH).next_back().map_or(0, |h| h.height), chain.first().map_or(0, |h| h.height)))
}

/// `prove`, from the first of several REST nodes (comma-separated) that gives a proof which checks.
pub(crate) fn prove_any(nodes: &str, chain: &[Header], ids: &[String], cost: &mut RestCost) -> Result<Proven, String> {
    let mut last = "no node given".to_string();
    for n in crate::nodes::each(nodes).iter().filter(|n| !n.starts_with("p2p://")) { match prove(n, chain, ids, cost) { Ok(x) => return Ok(x), Err(e) => last = e } }
    Err(last)
}

/// Why a signed transaction was not taken.
pub(crate) enum NotSent {
    /// A node read it and said no (its status and words): asking another node the same thing is not a retry.
    Refused(u16, String),
    /// No node could be reached at all: nothing left this machine.
    Unreachable(String),
    /// A node was reached and sent the transaction, and no answer came back (it timed out, the connection dropped,
    /// or it failed on its own account). It may have taken it. This is not "not sent": the caller keeps the
    /// payment as waiting, so the next sync settles it either way and nobody pays twice on a guess.
    Unknown(String),
}
/// Hand a signed transaction to the first REST node (of several, comma-separated) that answers. A node that cannot
/// be reached, or fails on its own account (5xx), is passed over for the next; a refusal is final.
/// Returns the node that took it and what it said (one line).
pub(crate) fn send_tx(nodes: &str, json: &str) -> Result<(String, String), NotSent> {
    let (mut last, mut may_have_gone) = ("no node given".to_string(), false);
    for n in crate::nodes::each(nodes).into_iter().filter(|n| !n.starts_with("p2p://")) {
        match http().post(&format!("{n}/transactions")).set("Content-Type", "application/json").send_string(json) {
            Ok(r) => return Ok((n, one_line(&r.into_string().unwrap_or_default()))),
            // once a node may hold it, another node's "already have it" or "double spend" is not a refusal of the payment
            Err(ureq::Error::Status(code, r)) if code < 500 && !may_have_gone => return Err(NotSent::Refused(code, r.into_string().unwrap_or_default())),
            Err(ureq::Error::Status(code, _)) => { may_have_gone = true; last = format!("{n} answered {code}"); }
            Err(ureq::Error::Transport(t)) => {
                // the connection was never made: nothing was sent to this node. Anything later than that may have been.
                if !matches!(t.kind(), ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Dns | ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme | ureq::ErrorKind::InvalidProxyUrl | ureq::ErrorKind::ProxyConnect) { may_have_gone = true; }
                last = format!("{n}: {t}");
            }
        }
    }
    Err(if may_have_gone { NotSent::Unknown(last) } else { NotSent::Unreachable(last) })
}

/// Box proof over the peer protocol. The node names the block its proof is against, so there is no
/// guessing: if that block is not in the agreed chain yet, its header (and up to two before it) is
/// requested by id and must link onto the agreed tip and pass the work checks.
pub(crate) fn prove_p2p(peer: &str, chain: &mut Vec<Header>, ids: &[String], cost: &mut RestCost) -> Result<Proven, String> {
    fresh_enough(chain, now_ms(), release_anchor().max_tip_age_ms)?;
    let keys = keys_of(ids)?;
    let rt = tokio::runtime::Handle::current();
    let bp = tokio::task::block_in_place(|| rt.block_on(tachys_net::fetch_box_proof(peer, &keys, Duration::from_secs(8))))?;
    cost.requests += 1; cost.sent += 4 + 32 * keys.len(); cost.received += bp.cost.bytes_received;
    if !chain.iter().any(|h| id32(h) == bp.header_id) {
        // fetch the named header and its recent ancestors by id; accept only if they link onto our tip
        let anchor = release_anchor();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let tip_id = chain.first().map(id32).ok_or("no agreed chain")?;
        let (raw, c) = tokio::task::block_in_place(|| rt.block_on(tachys_net::fetch_linking(peer, bp.header_id, tip_id, 12,
            |b: &[u8]| Header::scorex_parse_bytes(b).ok().map(|h| parent32(&h)))))?;
        cost.requests += c.requests_sent; cost.received += c.bytes_received;
        let fresh: Vec<Header> = raw.iter().filter_map(|b| Header::scorex_parse_bytes(b).ok()).collect();
        let tip = chain.first().cloned().ok_or("no agreed chain")?;
        let cut = fresh.iter().position(|h| h.parent_id == tip.id).ok_or("the block the node names does not link onto the agreed tip within 12 blocks; run `tachys tips` again")?;
        let mut add: Vec<Header> = fresh[..=cut].to_vec();
        // the difficulty these may carry is bounded by `carries_on`, as in `extend_chain` (see there for the rounding window)
        if !carries_on(&tip, &add) || add.iter().any(|h| header_ok(h, &anchor, now).is_err()) { return Err("the node's newer headers failed the link, work or difficulty checks".into()) }
        say!("node is {} block(s) past the agreed tip; fetched those headers by id over the peer protocol (linked, work-checked)", add.len());
        add.append(chain);
        *chain = add;
    }
    let h = chain.iter().find(|h| id32(h) == bp.header_id).ok_or("named header missing after extension")?;
    let root: Vec<u8> = h.state_root.into();
    let found = lookup_all(&root, &bp.proof, &keys).ok_or("the proof does not answer for every id asked, or does not reproduce the state root of the block the node named")?;
    Ok((h.height, found))
}

/// How far back from the stored chain a block is linked by hash, at most (about a week of mainnet blocks).
const LINK_BACK_MAX: u32 = 5000;

/// Headers as a node's REST port gives them, each rebuilt from its own bytes: an id is computed, never taken from the JSON.
fn headers_from(text: &str) -> Vec<Header> {
    serde_json::from_str::<Vec<Header>>(text).unwrap_or_default().into_iter().filter_map(|h| h.scorex_serialize_bytes().ok()).filter_map(|b| Header::scorex_parse_bytes(&b).ok()).collect()
}

/// Does this Merkle proof (the levels a node gave: sibling hash in hex, or empty, and its side) show that `leaf` is
/// under `root`? The leaf is ours (the transaction id asked about), never the one the node names.
pub(crate) fn merkle_ok(leaf: &[u8; 32], levels: &[(String, u64)], root: &[u8]) -> bool {
    use ergo_merkle_tree::{LevelNode, MerkleProof, NodeSide};
    let nodes: Option<Vec<LevelNode>> = levels.iter().map(|(hash, side)| {
        let side = NodeSide::try_from(u8::try_from(*side).ok()?).ok()?;
        if hash.is_empty() { return Some(LevelNode::empty_node(side)) }
        Some(LevelNode::new(ergo_chain_types::Digest32::from(<[u8; 32]>::try_from(unhex(hash)?).ok()?), side))
    }).collect();
    // a tree of transactions is never deeper than this; a longer proof is not one
    nodes.is_some_and(|n| n.len() <= 64 && MerkleProof::new(leaf, &n).valid(root))
}

/// Is this transaction in the chain the peers agreed on? Proven, not reported:
///  1. a node names the block it says holds the transaction (its word so far);
///  2. that block's header is found on the stored chain, or reached from the stored chain's oldest header by
///     following parent ids back, each fetched header hashing to the id the one above it names. A header reached
///     that way is an ancestor of an agreed header, so it is on the agreed chain, with the work that implies;
///  3. the node's Merkle proof must lead from the transaction's id to that header's transactions root.
///
/// Returns the height of the block. An `Err` means it could not be shown, not that it is not there: no proof can
/// show a transaction is absent.
pub(crate) fn prove_included(nodes: &str, chain: &[Header], tx: &str, cost: &mut RestCost) -> Result<u32, String> {
    fresh_enough(chain, now_ms(), release_anchor().max_tip_age_ms)?;
    included_in(nodes, chain, tx, cost)
}
/// `prove_included`, against whatever chain it is given.
pub(crate) fn included_in(nodes: &str, chain: &[Header], tx: &str, cost: &mut RestCost) -> Result<u32, String> {
    let leaf: [u8; 32] = unhex(tx).and_then(|b| b.try_into().ok()).ok_or("a transaction id is 32 bytes of hex")?;
    let oldest = chain.last().ok_or("no agreed chain")?;
    let get = |url: String, cost: &mut RestCost| -> Option<String> { cost.requests += 1; let t = http().get(&url).call().ok()?.into_string().ok()?; cost.received += t.len(); Some(t) };
    let mut last = "no node given".to_string();
    for n in crate::nodes::each(nodes).iter().filter(|n| !n.starts_with("p2p://")) {
        let Some(found) = get(format!("{n}/blockchain/transaction/byId/{tx}"), cost).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { last = format!("{n} knows no such transaction in a block"); continue };
        let (Some(block), Some(height)) = (found["blockId"].as_str().and_then(unhex).and_then(|b| <[u8; 32]>::try_from(b).ok()), found["inclusionHeight"].as_u64().and_then(|h| u32::try_from(h).ok())) else { last = format!("{n} did not name a block"); continue };
        if height > chain[0].height { last = format!("{n} names a block above the stored chain; run `tachys tips` again"); continue }
        // the header of that block, as a header of the agreed chain
        let header: Header = if height >= oldest.height {
            match chain.iter().find(|h| h.height == height) { Some(h) if id32(h) == block => h.clone(), _ => { last = format!("the block {n} names is not the agreed chain's block at height {height}"); continue } }
        } else {
            if oldest.height - height > LINK_BACK_MAX { last = format!("the block is {} blocks behind the stored chain, further than is followed back ({LINK_BACK_MAX})", oldest.height - height); continue }
            // parents, newest first, in slices; each must be the parent the one above names
            let (mut expect, mut at, mut reached) = (parent32(oldest), oldest.height, None);
            'back: while at > height {
                let from = at.saturating_sub(200).max(height);
                let Some(text) = get(format!("{n}/blocks/chainSlice?fromHeight={from}&toHeight={}", at - 1), cost) else { break };
                let mut slice = headers_from(&text);
                slice.sort_by_key(|h| std::cmp::Reverse(h.height));
                if slice.is_empty() { break }
                for h in slice {
                    if h.height != at - 1 || id32(&h) != expect { break 'back }
                    expect = parent32(&h); at = h.height;
                    if at == height { reached = Some(h); break 'back }
                }
            }
            match reached { Some(h) if id32(&h) == block => h, Some(_) => { last = format!("the block {n} names is not the agreed chain's block at height {height}"); continue }, None => { last = format!("{n} did not give headers that link back from the stored chain to height {height}"); continue } }
        };
        let Some(levels) = get(format!("{n}/blocks/{}/proofFor/{tx}", crate::util::hex(&block)), cost).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|p| p["levels"].as_array().map(|l| l.iter().filter_map(|x| Some((x.get(0)?.as_str()?.to_string(), x.get(1)?.as_u64()?))).collect::<Vec<_>>())) else { last = format!("{n} gave no proof for the transaction"); continue };
        let root: Vec<u8> = header.transaction_root.into();
        if merkle_ok(&leaf, &levels, &root) { return Ok(height) }
        last = format!("the proof {n} gave does not lead to the transactions root of the block at height {height}");
    }
    Err(last)
}

/// tachys included --state DIR [--node URL] --tx TXID: prove a transaction is in the agreed chain.
pub(crate) fn included(args: &[String]) -> crate::out::Ended {
    let state: String = arg(args, "--state", "state".to_string())?;
    let node = crate::nodes::resolved(&arg(args, "--node", crate::nodes::AUTO.to_string())?, &state)?;
    let tx: String = arg(args, "--tx", String::new())?;
    if tx.len() != 64 { return Err(crate::out::Fail::new(2, "usage: tachys included --state DIR [--node URL] --tx TXID".into())) }
    let chain = load_chain(&state);
    let mut cost = RestCost::default();
    crate::out::set("tx", &tx);
    match prove_included(&node, &chain, &tx, &mut cost) {
        Ok(height) => {
            let agreed = crate::chain::agreed_height(&state);
            crate::out::set("included", true); crate::out::set("height", height); crate::out::set("agreed_height", agreed);
            crate::out::set("confirmations", (agreed + 1).saturating_sub(height));
            say!("transaction {tx}: PROVEN in the block at height {height} of the agreed chain ({} confirmations); {} requests, {} B received", (agreed + 1).saturating_sub(height), cost.requests, cost.received);
            Ok(())
        }
        Err(why) => { crate::out::set("included", false); say!("transaction {tx}: NOT SHOWN to be in the chain: {why}"); Err(crate::out::Fail::said(1)) }
    }
}

/// What a carrier can check about someone else's payment once it is online: the inputs exist and
/// are unspent (box proof against the agreed chain), and the spending proofs verify against them.
/// Returns the height the inputs were proven at, or the reason to refuse.
/// An `Err` carries whether the refusal is for good (`true`: the payment can be dropped) or may pass (`false`: a
/// network failure, or something seen only in a block above `agreed`, which settles nothing).
pub(crate) fn check_parcel(peer: &str, chain: &mut Vec<Header>, agreed: u32, tx: &ergo_lib::chain::transaction::Transaction) -> Result<u32, (String, bool)> {
    use ergo_chain_types::PreHeader;
    use ergo_lib::chain::ergo_state_context::ErgoStateContext;
    use ergo_lib::chain::parameters::Parameters;
    use ergo_lib::wallet::signing::TransactionContext;
    use ergotree_ir::chain::ergo_box::ErgoBox;
    use ergotree_ir::serialization::SigmaSerializable;
    if chain.len() < 10 { return Err(("the carrier has no agreed chain; run `tachys tips` first".into(), false)) }
    let inputs: Vec<String> = tx.inputs.iter().map(|i| String::from(i.box_id)).collect();
    let data: Vec<String> = tx.data_inputs.as_ref().map(|d| d.iter().map(|i| String::from(i.box_id)).collect()).unwrap_or_default();
    let outputs: Vec<String> = tx.outputs.iter().map(|o| String::from(o.box_id())).collect();
    let mut ids = inputs.clone(); ids.extend(data.clone()); ids.extend(outputs.clone());
    let mut cost = RestCost::default();
    let (height, found) = prove_p2p(peer, chain, &ids, &mut cost).map_err(|e| (e, false))?;
    let settled = height <= agreed;
    let (fi, rest) = found.split_at(inputs.len());
    let (fd, fo) = rest.split_at(data.len());
    if fo.iter().any(|o| o.is_some()) { return Err((format!("already on chain (an output exists at height {height})"), settled)) }
    if let Some(i) = fi.iter().position(|b| b.is_none()) { return Err((format!("input {} is not an unspent box at height {height}", &inputs[i][..16]), settled)) }
    if fd.iter().any(|b| b.is_none()) { return Err(("a data input is not an unspent box".into(), settled)) }
    let parse = |v: &[Option<Vec<u8>>]| -> Result<Vec<ErgoBox>, String> { v.iter().map(|b| ErgoBox::sigma_parse_bytes(b.as_ref().expect("checked present")).map_err(|e| format!("box decode: {e:?}"))).collect() };
    let (boxes, dboxes) = (parse(fi).map_err(|e| (e, true))?, parse(fd).map_err(|e| (e, true))?);
    let headers: [Header; 10] = chain[..10].to_vec().try_into().map_err(|_| ("header context".to_string(), false))?;
    let ctx = ErgoStateContext::new(PreHeader::from(headers[0].clone()), headers, Parameters::default());
    let tc = TransactionContext::new(tx.clone(), boxes, dboxes).map_err(|e| (format!("inputs do not match the transaction: {e:?}"), true))?;
    tc.validate(&ctx).map_err(|e| (format!("spending proof or balance check failed: {e:?}"), true))?;
    Ok(height)
}

/// The node has moved past the agreed chain (routine on a fast chain). Ask it for its last headers and
/// extend the chain by at most `max` headers that link onto the agreed tip and pass the same
/// work, floor and time checks. These newest headers rest on one source plus their own work.
///
/// The floor that matters here is not the release's (`header_ok`) but `continues`: each header must carry the
/// difficulty of the one it follows, or at an epoch's first block at least half of it, less the compact form's rounding
/// (one part in 2^15). Half is also what the floor `tachys tips` remembers allows in the first epoch-time after a tip
/// (`remembered_floor`), so a header taken here can sit below that floor only inside the rounding allowance, which is
/// where an honest fall of the largest size the chain's rule allows lands too. That window is allowed on purpose.
pub(crate) fn extend_chain(node: &str, chain: &mut Vec<Header>, max: usize, cost: &mut RestCost) -> usize {
    let anchor = release_anchor();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    cost.requests += 1;
    let Ok(r) = http().get(&format!("{node}/blocks/lastHeaders/{}", max + 3)).call() else { return 0 };
    let text = r.into_string().unwrap_or_default();
    cost.received += text.len();
    // ids are recomputed from the bytes, never taken from the JSON
    let fresh: Vec<Header> = serde_json::from_str::<Vec<Header>>(&text).unwrap_or_default().into_iter()
        .filter_map(|h| h.scorex_serialize_bytes().ok()).filter_map(|b| Header::scorex_parse_bytes(&b).ok()).collect();
    let mut added = 0;
    while added < max {
        let Some(tip) = chain.first().cloned() else { break };
        match fresh.iter().find(|h| tachys_verify::tip::continues(&tip, h, chain_params().epoch_length) && header_ok(h, &anchor, now).is_ok()) {
            Some(h) => { chain.insert(0, h.clone()); added += 1; }
            None => break,
        }
    }
    added
}
