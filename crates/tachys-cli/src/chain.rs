//! The chain kept in the state directory, and the checks a single header must pass to join it.

use ergo_chain_types::Header;
use sigma_ser::ScorexSerializable;
use tachys_verify::Anchor;

use crate::network::chain_params;

/// A proof is taken only against one of this many newest headers of the stored chain, so what it shows is at most
/// a few blocks old. (An honest node is often a block behind or ahead; one further back than this is asked again later.)
pub(crate) const FRESH: usize = 4;

/// The height of the tip the peers last agreed on. Headers above it in the stored chain were pushed by fewer
/// network groups than the quorum, or added by the one node asked for a proof: real work, one source.
pub(crate) fn agreed_height(state: &str) -> u32 {
    std::fs::read(format!("{state}/agreed-tip.bin")).ok().and_then(|b| Header::scorex_parse_bytes(&b).ok()).map_or(0, |h| h.height)
}

pub(crate) fn load_chain(state: &str) -> Vec<Header> {
    let raw = std::fs::read(format!("{state}/agreed-chain.bin")).unwrap_or_default();
    let (mut chain, mut i) = (vec![], 0usize);
    while i + 2 <= raw.len() {
        let n = u16::from_be_bytes([raw[i], raw[i + 1]]) as usize; i += 2;
        if let Ok(h) = Header::scorex_parse_bytes(&raw[i..(i + n).min(raw.len())]) { chain.push(h); }
        i += n;
    }
    chain
}

/// The chain as it is kept on disk: newest first, each header as a two-byte length and its bytes.
pub(crate) fn chain_bytes<'a>(headers: impl Iterator<Item = &'a Header>) -> Vec<u8> {
    let mut out = vec![];
    for h in headers { if let Ok(b) = h.scorex_serialize_bytes() { out.extend_from_slice(&(b.len() as u16).to_be_bytes()); out.extend_from_slice(&b); } }
    out
}

/// A proof is read only against a stored chain whose newest header is recent (the same limit `tachys tips` holds a
/// tip to). A state directory left for days still holds a chain that was agreed on then: a box proven unspent
/// against it may have been spent since.
pub(crate) fn fresh_enough(chain: &[Header], now_ms: u64, max_age_ms: u64) -> Result<(), String> {
    let Some(top) = chain.first() else { return Err("no agreed chain; run `tachys tips` first".into()) };
    let age = now_ms.saturating_sub(top.timestamp);
    if age > max_age_ms { return Err(format!("the stored chain is {} min old (its newest header is at height {}), past the {} min a proof may rest on; run `tachys tips` again", age / 60_000, top.height, max_age_ms / 60_000)) }
    Ok(())
}
pub(crate) fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }

pub(crate) fn id32(h: &Header) -> [u8; 32] { let v: Vec<u8> = h.id.0.into(); let mut a = [0u8; 32]; a.copy_from_slice(&v); a }
pub(crate) fn parent32(h: &Header) -> [u8; 32] { let v: Vec<u8> = h.parent_id.0.into(); let mut a = [0u8; 32]; a.copy_from_slice(&v); a }

/// Checks a pushed or fetched header on its own: work, floor, not from the future.
/// Headers a single source adds on top of the agreed tip (`add`, newest first): each must follow the one before at
/// the chain's own difficulty, not merely above the floor.
pub(crate) fn carries_on(tip: &Header, add: &[Header]) -> bool {
    let epoch = chain_params().epoch_length;
    add.last().is_none_or(|first| tachys_verify::tip::continues(tip, first, epoch)) && add.windows(2).all(|w| tachys_verify::tip::continues(&w[1], &w[0], epoch))
}

pub(crate) fn header_ok(h: &Header, anchor: &Anchor, now: u64) -> Result<(), String> {
    if !matches!(h.check_pow(), Ok(true)) { return Err("work".into()) }
    if h.height <= anchor.checkpoint_height { return Err("at or below checkpoint".into()) }
    if tachys_verify::difficulty::difficulty(h) < anchor.floor_at(now) { return Err("below floor".into()) }
    if h.timestamp > now + anchor.max_future_drift_ms { return Err("future".into()) }
    Ok(())
}

/// `header_ok`, and at least `floor` difficulty: the release's floor or what this wallet remembers, whichever is higher.
pub(crate) fn header_ok_above(h: &Header, anchor: &Anchor, now: u64, floor: &num_bigint::BigInt) -> Result<(), String> {
    header_ok(h, anchor, now)?;
    if tachys_verify::difficulty::difficulty(h) < *floor { return Err("below the remembered floor".into()) }
    Ok(())
}
