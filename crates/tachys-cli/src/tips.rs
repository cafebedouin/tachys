//! tachys tips: the tip the peers agree on, from the headers they push unprompted.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ergo_chain_types::Header;
use sigma_ser::ScorexSerializable;
use tokio::sync::Semaphore;

use crate::chain::{carries_on, chain_bytes, header_ok_above, id32, parent32};
use crate::network::{chain_params, checkpoint_header, net, release_anchor};
use crate::util::{arg, group_of, hex, id_hex, private_dir, write_whole};

/// tachys tips: learn the chain tip from what peers push unprompted, then fetch a short linked chain
/// behind the heaviest tip and count how many peers' tips sit on it. No proof is requested.
/// Tips are ranked by the difficulty they claim, not their height (`tachys_verify::tip`).
/// Exits 0 only when a tip is agreed on (and, with `--state`, saved); 1 when none is; 2 on misuse.
pub(crate) async fn tips(args: &[String]) -> crate::out::Ended {
    crate::out::set("verdict", "not_trusted");
    // a build's first-run protection wears down with its checkpoint's age: said on every sync, and warned of once it is old
    let age = crate::network::checkpoint_age_days();
    crate::out::set("checkpoint_age_days", age);
    if age > crate::network::CHECKPOINT_OLD_DAYS && !crate::network::private_net() { say!("note: this build's checkpoint is {age} days old; the difficulty floor a first sync relies on has halved {} times since. Take a newer build.", age / 30); }
    // testnet has few peers: what suits mainnet (six peers, a quorum of three network groups) never agrees there
    let small = net().name == "testnet";
    let want: usize = arg(args, "--peers", if small { 2 } else { 6 })?;
    let tries: usize = arg(args, "--tries", if small { 8 } else { 16 })?;
    let wait: u64 = arg(args, "--wait", 6)?;
    let depth: usize = arg(args, "--depth", 10)?;
    let quorum: usize = arg(args, "--quorum", if small { 2 } else { 3 })?;
    // none of these may be nothing: no peers is a wait that never ends, and a quorum or a depth of nothing agrees to anything
    if want == 0 || tries == 0 || quorum == 0 || depth == 0 { return Err(crate::out::Fail::new(2, "--peers, --tries, --quorum and --depth are each at least 1".into())) }
    let seeds: Vec<String> = crate::network::seeds(args).map_err(|e| crate::out::Fail::new(2, e.to_string()))?.into_iter().take(tries).collect();
    let anchor = release_anchor();
    let t0 = Instant::now();
    let quiet: u64 = arg(args, "--quiet-ms", 1500)?;
    let state: String = arg(args, "--state", String::new())?;
    // a header we verified on an earlier run, if any: offered to silent peers straight away
    let stored: Option<Vec<u8>> = if state.is_empty() { None } else { std::fs::read(format!("{state}/behind-header.bin")).ok() };
    let from_state = stored.is_some();
    // the newest header this wallet agreed on in an earlier run: new tips may not claim much less difficulty than it did
    let remembered: Option<Header> = if state.is_empty() { None } else { std::fs::read(format!("{state}/agreed-tip.bin")).ok().and_then(|b| Header::scorex_parse_bytes(&b).ok()) };
    let floor = {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let release = anchor.floor_at(now);
        match &remembered { Some(h) => release.max(tachys_verify::tip::remembered_floor(h, now, &chain_params())), None => release }
    };
    // first run: the release's own checkpoint header is genuine and behind the tip, so it is safe to offer
    let stored = stored.or_else(|| checkpoint_header().scorex_serialize_bytes().ok());
    say!("tachys tips ({}): {} candidate peers, want {}, quiet {} ms, wait {} s, header to offer: {}", net().name, seeds.len(), want, quiet, wait, if from_state { "stored from last run" } else { "release checkpoint" });
    say!("least difficulty a header may claim: {floor} ({})", if remembered.is_some() { "from the tip agreed on last run, or the release floor if higher" } else { "release floor; no earlier tip remembered" });
    let (btx, brx) = tokio::sync::watch::channel(stored);
    let (tx, mut rx) = tokio::sync::mpsc::channel(seeds.len().max(1));
    // At most `want` connections open at once; once enough tips are in, no new connection starts,
    // and the ones in flight are allowed to finish so their cost is counted.
    let mut tasks = vec![];
    let slots = Arc::new(Semaphore::new(want));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    for addr in seeds.clone() {
        let (tx, brx, slots, stop) = (tx.clone(), brx.clone(), slots.clone(), stop.clone());
        tasks.push(tokio::spawn(async move {
            let _slot = slots.acquire().await;
            if stop.load(std::sync::atomic::Ordering::Relaxed) { return }
            let r = tachys_net::learn_tip(&addr, Duration::from_millis(quiet), Duration::from_secs(wait), brx).await;
            let _ = tx.send((addr, r)).await;
        }));
    }
    drop(tx);
    let (mut pushed, mut bytes, mut held, mut requests, mut conns) = (vec![], 0usize, 0u128, 0usize, 0usize);
    while let Some((addr, r)) = rx.recv().await {
        match r {
            Err((e, cost)) => { conns += (cost.connect_ms > 0) as usize; bytes += cost.bytes_received; held += cost.held_ms; requests += cost.requests_sent; say!("  {addr:22} -- {e} (held {} ms)", cost.held_ms); }
            Ok(tp) => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                conns += 1; bytes += tp.cost.bytes_received; held += tp.cost.held_ms; requests += tp.cost.requests_sent;
                let hs: Vec<Header> = tp.headers.iter().filter_map(|b| Header::scorex_parse_bytes(b).ok()).collect();
                let good: Vec<&Header> = hs.iter().filter(|h| header_ok_above(h, &anchor, now, &floor).is_ok()).collect();
                // The peer's tip: the highest of its headers that follows another it pushed (its parent, at a difficulty
                // the chain's rule allows after it). A header that follows none of them, higher than the rest, is not
                // taken for its tip on its height alone. Not "the heaviest": after an honest fall in difficulty the
                // newest header is lighter than its ancestors. A peer whose headers follow none of each other (the
                // reference node pushes tip, tip-16, tip-128 and tip-512) has its highest taken, as before.
                let follows = |h: &&&Header| good.iter().any(|p| tachys_verify::tip::continues(p, h, chain_params().epoch_length));
                let tip = good.iter().filter(follows).max_by_key(|h| h.height).or_else(|| good.iter().max_by_key(|h| h.height));
                say!("  {addr:22} {} {} {} after {} ms, {} B, {} headers ({} pass), tip {}{}", tp.agent, tp.version, if tp.provoked { "answered" } else { "pushed" },
                    tp.first_sync_ms, tp.sync_frame_bytes, hs.len(), good.len(), tip.map_or(0, |h| h.height),
                    if tp.extension_ids > 0 { format!(", announced {} ids", tp.extension_ids) } else { String::new() });
                if let Some(tip) = tip {
                    // offer silent peers a verified header that is behind this tip: the one about 16 back if
                    // the peer sent it, never the tip itself
                    // always switch to the most recent such header: offering an old one (the release
                    // checkpoint on a first run) makes each peer announce up to 400 header ids, about 13 KB
                    if let Some(b) = good.iter().filter(|h| h.height + 8 <= tip.height).max_by_key(|h| h.height) {
                        if let Ok(bytes) = b.scorex_serialize_bytes() { let _ = btx.send(Some(bytes)); }
                    }
                    pushed.push((addr.clone(), (*tip).clone(), hs.clone()));
                }
                if pushed.len() >= want { stop.store(true, std::sync::atomic::Ordering::Relaxed); }
            }
        }
    }
    drop(tasks);
    if pushed.is_empty() { say!("no peer pushed a usable tip\nverdict: NOT TRUSTED (no tip heard)"); return Err(crate::out::Fail::said(1)) }
    // Try the pushed tips heaviest first. A header's height is its maker's word; the difficulty it was mined at is not.
    use tachys_verify::tip::{candidates, judge, TipView};
    let views: Vec<TipView> = pushed.iter().map(|(addr, tip, _)| TipView { group: group_of(addr), tip: tip.clone() }).collect();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let pool: Vec<&Header> = { let mut p: Vec<&Header> = pushed.iter().flat_map(|p| p.2.iter()).collect(); p.sort_by_key(|h| std::cmp::Reverse(h.height)); p };
    let mut tried: Vec<Header> = vec![];
    let mut agreed: Option<(Vec<Header>, tachys_verify::tip::Agreement)> = None;
    let mut refusals: Vec<String> = vec![];
    // how far above the agreed tip a pushed header is followed, and how far back a heavier tip is looked for among this chain's ancestors
    const AHEAD_MAX: usize = 4;
    const BACK_MAX: usize = 128;
    for top in candidates(&views) {
        // a tip already on a chain that was refused has nothing new to offer; three chains are enough to ask for
        if tried.iter().any(|h| h.id == top.id) { continue }
        if refusals.len() >= 3 { break }
        // every peer that pushed this tip can serve its chain: if the first does not, the next is asked, so one peer
        // dropping the connection does not cost the tip
        let mut sources = pushed.iter().filter(|p| p.1.id == top.id).map(|p| p.0.clone());
        let Some(mut src) = sources.next() else { continue };
        let mut chain = vec![top.clone()];
        let mut older: Vec<Header> = vec![];
        let (mut want, mut rounds) = (depth, 0);
        let verdict = loop {
            rounds += 1;
            let had = chain.len();
            // a peer that pushed many consecutive headers has already given us the chain: use it
            while chain.len() < want {
                let want_id = parent32(chain.last().expect("non-empty"));
                match pool.iter().find(|h| id32(h) == want_id && header_ok_above(h, &anchor, now, &floor).is_ok()) { Some(h) => chain.push((*h).clone()), None => break }
            }
            if chain.len() >= want {
                if rounds == 1 || chain.len() > had { say!("chain: {} linked headers {}..{} taken from pushed headers; no extra connection", chain.len(), chain.last().map(|h| h.height).unwrap_or(0), top.height); }
            } else {
                let have = chain.len();
                match tachys_net::fetch_chain_back(&src, parent32(chain.last().expect("non-empty")), want - have, Duration::from_secs(8),
                    |b| Header::scorex_parse_bytes(b).ok().map(|h| parent32(&h))).await {
                    Err(e) => { say!("chain fetch from {src} failed: {e}"); if let Some(next) = sources.next() { src = next; continue } }
                    Ok((raw, cost)) => {
                        conns += 1; bytes += cost.bytes_received; held += cost.held_ms; requests += cost.requests_sent;
                        for b in raw {
                            match Header::scorex_parse_bytes(&b) {
                                Ok(h) if id32(&h) == parent32(chain.last().expect("non-empty")) && header_ok_above(&h, &anchor, now, &floor).is_ok() => chain.push(h),
                                _ => { say!("chain fetch: a header did not link or failed its checks; stopping"); break }
                            }
                        }
                        say!("chain from {src}: {} linked headers {}..{} ({} already in hand) in {} requests, {} B, {} ms", chain.len(), chain.last().map(|h| h.height).unwrap_or(0), top.height, have, cost.requests_sent, cost.bytes_received, cost.held_ms);
                    }
                }
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
            match judge(&chain, &views, &older, depth, quorum, chain_params().epoch_length, now, anchor.max_tip_age_ms) {
                // Its newest headers are ahead of what a quorum has reached, so the agreed tip sits lower and more headers
                // are wanted below it. Ask once more, if we got all we asked for last time.
                Err(tachys_verify::tip::Refusal::Short { want: w, .. }) if w > want && w <= depth + AHEAD_MAX && chain.len() >= want && rounds < 6 => { want = w; }
                // A heavier tip was heard that is not on this chain. It may be this chain's own past, from before difficulty
                // fell: ask for the ancestors down to its height. Each is the parent of the one before, so they are linked
                // by hash to headers already checked; if the tip is among them it is explained, and if not it stands.
                Err(tachys_verify::tip::Refusal::HeavierHeard { height }) if rounds < 6 => {
                    let low = older.last().or(chain.last()).expect("non-empty").clone();
                    let gap = low.height.saturating_sub(height) as usize;
                    if gap == 0 || older.len() + gap > BACK_MAX { break Err(tachys_verify::tip::Refusal::HeavierHeard { height }) }
                    let Ok((raw, cost)) = tachys_net::fetch_chain_back(&src, parent32(&low), gap, Duration::from_secs(8), |b| Header::scorex_parse_bytes(b).ok().map(|h| parent32(&h))).await else { break Err(tachys_verify::tip::Refusal::HeavierHeard { height }) };
                    conns += 1; bytes += cost.bytes_received; held += cost.held_ms; requests += cost.requests_sent;
                    let before = older.len();
                    for b in raw {
                        let expect = parent32(older.last().or(chain.last()).expect("non-empty"));
                        match Header::scorex_parse_bytes(&b) { Ok(h) if id32(&h) == expect => older.push(h), _ => break }
                    }
                    say!("a heavier tip at height {height} was heard; fetched {} ancestor(s) of this chain from {src} to see whether it is one of them ({} B)", older.len() - before, cost.bytes_received);
                    if older.len() == before { break Err(tachys_verify::tip::Refusal::HeavierHeard { height }) }
                }
                other => break other,
            }
        };
        match verdict {
            Ok(a) => { agreed = Some((chain, a)); break }
            Err(why) => {
                say!("tip at height {} (difficulty {}) not agreed on: {why:?}", top.height, tachys_verify::difficulty::difficulty(&top));
                refusals.push(format!("height {}: {why:?}", top.height));
                tried.extend(chain);
            }
        }
    }
    let on = |chain: &[Header], h: &Header| chain.iter().any(|c| c.id == h.id);
    let off: Vec<String> = match &agreed { Some((chain, _)) => pushed.iter().filter(|p| !on(chain, &p.1)).map(|p| format!("{}@{}", p.0, p.1.height)).collect(), None => vec![] };
    say!("\n{} peers pushed a tip; {} network groups have reached the agreed tip; off-chain: {:?}", pushed.len(), agreed.as_ref().map_or(0, |a| a.1.groups.len()), off);
    say!("neighbour cost: {} connections, {} messages sent, {:.1} connection-seconds held (failures included), {} B received, {:.1} s wall", conns, requests, held as f64 / 1000.0, bytes, t0.elapsed().as_secs_f64());
    crate::out::set("peers_heard", pushed.len()); crate::out::set("refusals", &refusals);
    let Some((chain, a)) = agreed else { say!("verdict: NOT TRUSTED ({})", refusals.join("; ")); return Err(crate::out::Fail::said(1)) };
    // The agreed tip is the newest header a quorum has reached. Headers above it were pushed by fewer groups: they are
    // kept on top of the stored chain so that a proof from a node already at one of them can be read, but only if each
    // follows at the chain's own difficulty, and nothing proven against them counts as confirmed.
    let top = chain[a.ahead].clone();
    let ahead: Vec<Header> = if carries_on(&top, &chain[..a.ahead]) { chain[..a.ahead].to_vec() } else { vec![] };
    let agreed_chain = &chain[a.ahead..];
    // Only an agreed chain is kept. Its oldest header is offered to peers next run (it is behind any tip); its newest
    // sets the floor for next time; the chain, newest first (u16 length + header bytes each), is what box proofs are checked against.
    // Nothing is called agreed until it is on disk: a caller that reads "Agreed" goes on to prove boxes against the
    // stored chain, and must not be left with the one from an earlier run. The chain goes first; each file is
    // written whole or not at all.
    if !state.is_empty() {
        private_dir(&state);
        let saved = write_whole(&format!("{state}/agreed-chain.bin"), &chain_bytes(ahead.iter().chain(agreed_chain.iter())))
            .and_then(|_| write_whole(&format!("{state}/agreed-tip.bin"), &top.scorex_serialize_bytes().map_err(std::io::Error::other)?))
            .and_then(|_| write_whole(&format!("{state}/behind-header.bin"), &agreed_chain.last().expect("non-empty").scorex_serialize_bytes().map_err(std::io::Error::other)?));
        if let Err(e) = saved { say!("verdict: NOT TRUSTED (a tip was agreed on but could not be saved in {state}: {e})"); return Err(crate::out::Fail::said(1)) }
    }
    let root: Vec<u8> = top.state_root.into();
    crate::out::set("verdict", "agreed"); crate::out::set("groups", a.groups.len()); crate::out::set("ahead", ahead.len()); crate::out::set("saved", !state.is_empty());
    crate::out::set("tip", serde_json::json!({ "height": top.height, "id": id_hex(&top), "timestamp_ms": top.timestamp, "age_s": now.saturating_sub(top.timestamp) / 1000, "state_root": hex(&root) }));
    say!("verdict: Agreed\ntip: height {} id {} ({} s old)\nstate root: {}", top.height, id_hex(&top), now.saturating_sub(top.timestamp) / 1000, hex(&root));
    if !ahead.is_empty() { say!("above it: {} header(s) up to height {} that fewer than {quorum} network groups have reached; kept for reading proofs, not agreed on", ahead.len(), ahead[0].height); }
    Ok(())
}
