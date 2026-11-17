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

    // the run a wallet would fetch: the last 1,152 headers, for every place the tip can have in its epoch
    let mut exact_runs = 0;
    for cut in 0..128 {
        let end = headers.len() - cut;
        let run = &headers[end - 1152..end];
        let r = check_run(run, &p).unwrap();
        assert!(r.wrong.is_empty());
        if cut == 0 || cut == 127 { show(&format!("last 1,152 ending {} blocks before the tip", cut), &r); }
        // the newest epoch start is recomputed, so the tip's own difficulty rests on headers of the run
        let newest_start = run.iter().map(|h| h.height).filter(|h| (h - 1) % 128 == 0).max().unwrap();
        assert_eq!(r.exact_from, Some(newest_start), "cut {cut}");
        exact_runs += 1;
    }
    println!("runs of 1,152 checked: {exact_runs}, each with its newest epoch recomputed from its own headers");

    // a run one header short of nine epochs can miss it
    let short: Vec<usize> = (0..128).filter(|cut| {
        let end = headers.len() - cut;
        check_run(&headers[end - 1151..end], &p).unwrap().exact_from.is_none()
    }).collect();
    println!("runs of 1,151 that recompute no epoch: {} of 128", short.len());
    assert!(!short.is_empty());

    // a difficulty the rule does not give is seen: change one epoch's declared difficulty by its smallest step
    let mut bent = headers[headers.len() - 1152..].to_vec();
    let start = bent.iter().position(|h| (h.height - 1) % 128 == 0 && h.height == check_run(&bent, &p).unwrap().exact_from.unwrap()).unwrap();
    let n_bits = bent[start].n_bits + 1;
    for h in bent[start..].iter_mut() { h.n_bits = n_bits; }
    // (ids and parent ids are left as they are: the check under test is the difficulty rule, not the hashes)
    let r = check_run(&bent, &p).unwrap();
    println!("one epoch declared one step off: wrong at {:?}", r.wrong);
    assert_eq!(r.wrong, vec![bent[start].height]);
}
