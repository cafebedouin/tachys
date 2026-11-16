use ergo_chain_types::Header;
use tachys_verify::difficulty::*;

#[test]
fn mainnet_epoch_1915777_recomputed_from_nine_headers() {
    let hs: Vec<Header> = serde_json::from_str(include_str!("../../../vectors/epoch-headers.json")).unwrap();
    let p = ChainParams::mainnet();
    assert_eq!(hs.len(), 10);
    let want: Vec<u32> = previous_heights_required(1_915_777, p.epoch_length, p.use_last_epochs);
    assert_eq!(want, hs[..9].iter().map(|h| h.height).collect::<Vec<_>>());
    let got = eip37_calculate(&hs[..9], &p).unwrap();
    println!("computed={} header_1915777={} previous_epoch={}", got, difficulty(&hs[9]), difficulty(&hs[8]));
    assert_eq!(got, difficulty(&hs[9]));
    assert_ne!(difficulty(&hs[8]), difficulty(&hs[9]), "control: the epoch really changed difficulty");
    // negative control: perturb one input timestamp and the result must move
    let mut bad = hs[..9].to_vec();
    bad[8].timestamp += 600_000;
    assert_ne!(eip37_calculate(&bad, &p).unwrap(), difficulty(&hs[9]));
}
