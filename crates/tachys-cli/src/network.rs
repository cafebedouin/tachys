//! The network this run is on: its constants, the checkpoint a release carries, and the anchor made from it.

use ergo_chain_types::autolykos_pow_scheme::decode_compact_bits;
use ergo_chain_types::{BlockId, Digest32, Header};
use tachys_verify::difficulty::ChainParams;
use tachys_verify::Anchor;

/// Per-network constants and the anchor a release would carry.
pub(crate) struct Net {
    pub(crate) name: &'static str,
    pub(crate) magic: [u8; 4],
    pub(crate) genesis: &'static str,
    /// the checkpoint header a release carries, as the node's JSON
    pub(crate) checkpoint_json: &'static str,
    pub(crate) floor_divisor: u32,
    pub(crate) v2_activation_height: u32,
    pub(crate) block_interval_ms: u64,
    /// Blocks between changes of difficulty: 128 on both public networks; a private network's own (the rig's devnet: 16).
    pub(crate) epoch_length: u32,
    pub(crate) seeds: &'static str,
    pub(crate) testnet: bool,
    /// Hex of the fee output's script when the network's reward delay differs from mainnet's (private networks).
    pub(crate) fee_tree: &'static str,
}
pub(crate) static MAINNET: Net = Net {
    name: "mainnet",
    magic: [1, 0, 2, 4],
    genesis: "b0244dfc267baca974a4caee06120321562784303a8a688976ae56170e4d175b",
    checkpoint_json: include_str!("../../../anchors/mainnet.json"),
    floor_divisor: 4,
    v2_activation_height: 417_792,
    block_interval_ms: 120_000,
    epoch_length: 128,
    seeds: "seeds-mainnet.txt",
    testnet: false,
    fee_tree: "",
};
/// Testnet difficulty swings widely, so its floor is loose.
pub(crate) static TESTNET: Net = Net {
    name: "testnet",
    magic: [2, 3, 2, 3],
    genesis: "5b1827ca092b599eafbaf339d2acf2445bc5216ec2e022d9c001a6fff660cad9",
    checkpoint_json: include_str!("../../../anchors/testnet.json"),
    floor_divisor: 1024,
    v2_activation_height: 0,
    block_interval_ms: 45_000,
    epoch_length: 128,
    seeds: "seeds-testnet.txt",
    testnet: true,
    fee_tree: "",
};
/// The network of the call under way (`Own::net`). Set at the start of every call, so one process can serve mainnet
/// and then testnet, and both at once.
pub(crate) fn net() -> &'static Net {
    *crate::own::own()
        .net
        .read()
        .unwrap_or_else(|p| p.into_inner())
}
pub(crate) fn set_net(n: &'static Net) {
    *crate::own::own()
        .net
        .write()
        .unwrap_or_else(|p| p.into_inner()) = n;
}

/// A network of our own (the rig), set up from `--net-file`: the only place the test hooks work.
pub(crate) fn private_net() -> bool {
    !matches!(net().name, "mainnet" | "testnet")
}

/// The peers to try. `--seeds FILE` names a list and must be readable; without it, mainnet and testnet use the list
/// shipped inside the binary and a private network uses the file its net file names. A file that merely happens to
/// sit in the current directory is never picked up: whoever writes the seed list chooses every peer.
/// One `host:port` a line; blank lines and lines starting with `#` are skipped.
pub(crate) fn seeds(args: &[String]) -> Result<Vec<String>, String> {
    let given: String = crate::util::try_arg(args, "--seeds", String::new())?;
    let text = if !given.is_empty() {
        std::fs::read_to_string(&given).map_err(|e| format!("--seeds {given}: {e}"))?
    } else {
        match net().name {
            "mainnet" => include_str!("../../../seeds-mainnet.txt").to_string(),
            "testnet" => include_str!("../../../seeds-testnet.txt").to_string(),
            _ => {
                std::fs::read_to_string(net().seeds).map_err(|e| format!("{}: {e}", net().seeds))?
            }
        }
    };
    Ok(seed_lines(&text))
}
pub(crate) fn seed_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

pub(crate) fn checkpoint_header() -> Header {
    serde_json::from_str(net().checkpoint_json)
        .expect("checkpoint header (checked when the network was set)")
}
/// Is the checkpoint of the network just set a header? A release's own is; one from `--net-file` may not be.
pub(crate) fn checkpoint_readable() -> bool {
    serde_json::from_str::<Header>(net().checkpoint_json).is_ok()
}

pub(crate) fn release_anchor() -> Anchor {
    let n = net();
    let cp = checkpoint_header();
    Anchor {
        genesis_id: BlockId(Digest32::try_from(n.genesis.to_string()).expect("genesis id")),
        checkpoint_height: cp.height,
        checkpoint_difficulty: decode_compact_bits(cp.n_bits),
        checkpoint_timestamp_ms: cp.timestamp,
        floor_divisor: n.floor_divisor,
        floor_halving_ms: 30 * 24 * 3600 * 1000,
        min_k: 10,
        max_tip_age_ms: 3 * 3600 * 1000,
        max_future_drift_ms: 20 * 60 * 1000,
    }
}
/// How old the release's checkpoint is, in days. The difficulty floor halves every thirty of them, so this is how
/// far a build's first-run protection has worn down.
pub(crate) fn checkpoint_age_days() -> u64 {
    crate::chain::now_ms().saturating_sub(checkpoint_header().timestamp) / 86_400_000
}
/// Past this a build should be replaced by one with a newer checkpoint (`docs/maintaining.md`).
pub(crate) const CHECKPOINT_OLD_DAYS: u64 = 60;

pub(crate) fn chain_params() -> ChainParams {
    let mut p = ChainParams::mainnet();
    p.v2_activation_height = net().v2_activation_height;
    p.block_interval_ms = net().block_interval_ms;
    p.epoch_length = net().epoch_length;
    p
}
