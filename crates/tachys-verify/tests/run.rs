//! The recent run: difficulty recomputed over consecutive headers. On demand on a stretch of mainnet.
use ergo_chain_types::Header;
use tachys_verify::difficulty::ChainParams;
use tachys_verify::run::*;

/// `TACHYS_HEADERS_JSON=a.json:b.json cargo test --test run -- --ignored --nocapture`
#[test]
#[ignore]
fn on_a_stretch_of_mainnet() {
    let mut headers: Vec<Header> = vec![];
    for path in std::env::var("TACHYS_HEADERS_JSON").expect("TACHYS_HEADERS_JSON").split(':') {
        headers.extend(serde_json::from_str::<Vec<Header>>(&std::fs::read_to_string(path).unwrap()).unwrap());
    }
    headers.sort_by_key(|h| h.height);
    let p = ChainParams::mainnet();
    let show = |name: &str, r: &RunReport| println!(
        "{name}: heights {}..{} recomputed {} as-parent {} within-bounds {} wrong {:?} exact-from {:?} work {} work-exact {}",
        r.first_height, r.last_height, r.recomputed, r.as_parent, r.within_bounds, r.wrong, r.exact_from, r.work, r.work_exact);

    let whole = check_run(&headers, &p).expect("the stretch is consecutive");
    show("whole stretch", &whole);
    assert!(whole.wrong.is_empty());
    // every epoch start whose nine inputs are inside the stretch is recomputed; the earlier ones are not
    let starts: Vec<u32> = headers.iter().map(|h| h.height).filter(|h| (h - 1) % 128 == 0 && *h > headers[0].height).collect();
    let recomputable = starts.iter().filter(|h| **h - 1 - 8 * 128 >= headers[0].height).count();
    assert_eq!(whole.recomputed, recomputable);
    assert_eq!(whole.within_bounds, starts.len() - recomputable);
    assert_eq!(whole.recomputed + whole.as_parent + whole.within_bounds, headers.len() - 1);

    // the run a wallet would fetch, for every place the tip can have in its epoch
    let mut one_short = vec![];
    for cut in 0..128 {
        let end = headers.len() - cut;
        let tip = headers[end - 1].height;
        let newest_start = ((tip - 1) / 128) * 128 + 1;
        // exactly as many headers as the rule needs for this tip, and one fewer
        let need = run_length_for(tip, &p) as usize;
        assert_eq!(need as u32, tip - newest_start + 1026);
        let exact = check_run(&headers[end - need..end], &p).unwrap();
        assert!(exact.wrong.is_empty());
        assert_eq!(exact.exact_from, Some(newest_start), "cut {cut}: the newest epoch is recomputed from the run");
        assert_eq!(check_run(&headers[end - (need - 1)..end], &p).unwrap().exact_from, None, "cut {cut}: one header fewer is not enough");
        // nine epochs of 128, the first estimate, and one more
        if check_run(&headers[end - 1152..end], &p).unwrap().exact_from != Some(newest_start) { one_short.push(tip); }
        assert_eq!(check_run(&headers[end - 1153..end], &p).unwrap().exact_from, Some(newest_start), "cut {cut}: 1,153 always suffice");
        if cut == 0 { show("the run for the tip", &exact); println!("  it needs {need} headers"); }
    }
    println!("of 128 tips, 1,152 headers are one short at: {:?} (a tip that closes an epoch); 1,153 suffice at all 128", one_short);
    assert_eq!(one_short.len(), 1);
    assert_eq!(one_short[0] % 128, 0);

    // a difficulty the rule does not give is seen: change one epoch's declared difficulty by its smallest step
    let mut bent = headers[headers.len() - 1153..].to_vec();
    let start = bent.iter().position(|h| (h.height - 1) % 128 == 0 && h.height == check_run(&bent, &p).unwrap().exact_from.unwrap()).unwrap();
    let n_bits = bent[start].n_bits + 1;
    for h in bent[start..].iter_mut() { h.n_bits = n_bits; }
    // (ids and parent ids are left as they are: the check under test is the difficulty rule, not the hashes)
    let r = check_run(&bent, &p).unwrap();
    println!("one epoch declared one step off: wrong at {:?}", r.wrong);
    assert_eq!(r.wrong, vec![bent[start].height]);
}

#[test]
fn the_run_a_tip_needs_is_counted_without_overflow_at_any_height() {
    let p = ChainParams::mainnet();
    // the module's own figures: 1,026 headers when the tip opens an epoch, 1,153 when it closes one
    assert_eq!(run_length_for(129, &p), 1_026);
    assert_eq!(run_length_for(256, &p), 1_153);
    // height 0 (genesis, or a header claiming it) reads as height 1, not an overflow
    assert_eq!(run_length_for(0, &p), run_length_for(1, &p));
    assert_eq!(run_length_for(1, &p), 1_026);
    assert!(run_length_for(u32::MAX, &p) <= 1_153);
}
