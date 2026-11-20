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
    // A first run has only the release's floor to hold headers to, and ten headers at that floor cost a forger little
    // (finding 53). So a first run links the agreed chain back to the release's checkpoint, every header checked for
    // work and floor and each a step the chain's own difficulty rule allows, and holds the tip to the height the time
    // since the checkpoint implies, at half the chain's pace. A forger must then mine all of that, falling in difficulty
    // by at most half an epoch, never below the floor. A wallet that keeps the linked chain (`--keep-chain`, for one
    // that serves others) extends it every run.
    let keep = args.iter().any(|a| a == "--keep-chain");
    let first = remembered.is_none();
    // the chain from just above the checkpoint to just below the agreed chain, oldest first, once it is linked
    let mut linked: Vec<Header> = vec![];
    if first || keep {
        let low = chain.last().expect("non-empty").clone();
        let cp = checkpoint_header();
        // What was linked before is not fetched again: a kept chain (`--keep-chain`), or what a first run that stopped
        // part way linked (`linking.bin`). Like the stored chain, these are this wallet's own files, checked when written.
        let prior_file = if keep { "linked-chain.bin" } else { "linking.bin" };
        let mut prior: Vec<Header> = if state.is_empty() { vec![] } else { crate::chain::load_chain_file(&format!("{state}/{prior_file}")) };
        prior.reverse(); prior.retain(|h| h.height < low.height);
        let srcs: Vec<String> = pushed.iter().filter(|p| on(&chain, &p.1)).map(|p| p.0.clone()).chain(pushed.iter().map(|p| p.0.clone())).collect();
        let (t_link, bytes_before) = (Instant::now(), bytes);
        let mut res = link_forward(&srcs, prior.last().unwrap_or(&cp), &low, &anchor, &mut (&mut conns, &mut bytes, &mut held, &mut requests)).await;
        // what was linked before may be off the chain now (a reorganisation): once more, from the checkpoint. Only then:
        // a link that ran short (a slow peer, the time budget) keeps what it had and goes on from it next time
        if matches!(&res, Err((_, _, true))) && !prior.is_empty() { prior.clear(); res = link_forward(&srcs, &cp, &low, &anchor, &mut (&mut conns, &mut bytes, &mut held, &mut requests)).await; }
        let from_height = prior.last().unwrap_or(&cp).height;
        match res {
            Ok(between) => {
                say!("linked: {} header(s) from height {from_height} up to the agreed chain, each with work above the floor and a step the difficulty rule allows ({} B, {} ms)", between.len(), bytes - bytes_before, t_link.elapsed().as_millis());
                crate::out::set("linked", serde_json::json!({ "from_height": from_height, "headers": between.len(), "bytes": bytes - bytes_before, "ms": t_link.elapsed().as_millis() as u64 }));
                linked = prior.into_iter().chain(between).collect();
                if !state.is_empty() { let _ = std::fs::remove_file(format!("{state}/linking.bin")); }
            }
            Err((why, partial, _)) if first => {
                // what did link is kept, so that the next try goes on from it rather than from the checkpoint again
                let got = prior.len() + partial.len();
                if !state.is_empty() && got > 0 { private_dir(&state); let all: Vec<Header> = prior.into_iter().chain(partial).collect(); let _ = write_whole(&format!("{state}/linking.bin"), &chain_bytes(all.iter().rev())); }
                say!("verdict: NOT TRUSTED (a first run must link the chain to the release checkpoint: {why}; {got} header(s) linked so far are kept for the next try)");
                crate::out::set("code", "not_linked"); crate::out::set("refusals", vec![format!("not linked to the checkpoint: {why}")]);
                return Err(crate::out::Fail::said(1))
            }
            Err((why, _, _)) => say!("note: the kept chain could not be extended ({why}); it is dropped and rebuilt on the next run"),
        }
        if first {
            let least = cp.height as u64 + now.saturating_sub(cp.timestamp) / (2 * chain_params().block_interval_ms.max(1));
            if (chain[a.ahead].height as u64) < least {
                say!("verdict: NOT TRUSTED (tip at height {} is below {least}, half the height {} days since the checkpoint would bring)", chain[a.ahead].height, now.saturating_sub(cp.timestamp) / 86_400_000);
                crate::out::set("code", "below_pace"); crate::out::set("refusals", vec![format!("tip below the height the time since the checkpoint implies ({least})")]);
                return Err(crate::out::Fail::said(1))
            }
        }
    }
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
        // the whole chain from the checkpoint, oldest last, for a wallet that serves others their first run
        if keep {
            let path = format!("{state}/linked-chain.bin");
            if linked.is_empty() { let _ = std::fs::remove_file(&path); }
            else { let _ = write_whole(&path, &chain_bytes(agreed_chain.iter().chain(linked.iter().rev()))); }
        }
    }
    let root: Vec<u8> = top.state_root.into();
    crate::out::set("verdict", "agreed"); crate::out::set("groups", a.groups.len()); crate::out::set("ahead", ahead.len()); crate::out::set("saved", !state.is_empty());
    crate::out::set("tip", serde_json::json!({ "height": top.height, "id": id_hex(&top), "timestamp_ms": top.timestamp, "age_s": now.saturating_sub(top.timestamp) / 1000, "state_root": hex(&root) }));
    say!("verdict: Agreed\ntip: height {} id {} ({} s old)\nstate root: {}", top.height, id_hex(&top), now.saturating_sub(top.timestamp) / 1000, hex(&root));
    if !ahead.is_empty() { say!("above it: {} header(s) up to height {} that fewer than {quorum} network groups have reached; kept for reading proofs, not agreed on", ahead.len(), ahead[0].height); }
    Ok(())
}

/// Headers forward from `from` (not included) to the parent of `to` (not included), from the first of `srcs` that
/// gives them, each checked on its own (work, floor, clock) and as a step from the one before (the next height, its
/// parent, a difficulty the rule allows), the last the parent of `to`. A source that stops short is followed by the next,
/// from the last header that passed.
/// On failure, what did link comes back with the reason. Each source has five minutes, the whole fifteen.
/// The third part of a failure says the start itself is off the chain: nothing followed it, or what did does not lead
/// to `to`.
async fn link_forward(srcs: &[String], from: &Header, to: &Header, anchor: &tachys_verify::Anchor, cost: &mut (&mut usize, &mut usize, &mut u128, &mut usize)) -> Result<Vec<Header>, (String, Vec<Header>, bool)> {
    let end = Instant::now() + Duration::from_secs(900);
    use crate::chain::header_ok;
    let epoch = chain_params().epoch_length;
    let want = to.height.saturating_sub(from.height).saturating_sub(1) as usize;
    let mut out: Vec<Header> = vec![];
    let mut last = from.clone();
    let mut tried = std::collections::BTreeSet::new();
    // a header came that does not follow `from` itself: the start is off the chain those peers are on
    let mut off_at_start = false;
    for src in srcs {
        if out.len() >= want || Instant::now() >= end { break }
        if !tried.insert(src.clone()) { continue }
        let Ok(bytes) = last.scorex_serialize_bytes() else { break };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let budget = Duration::from_secs(300).min(end.saturating_duration_since(Instant::now()));
        match tachys_net::sync_forward(src, bytes, want - out.len(), Duration::from_secs(10), budget, |b| Header::scorex_parse_bytes(b).ok().map(|h| id32(&h)), |b| Header::scorex_parse_bytes(b).ok().map(|h| parent32(&h))).await {
            Err(e) => say!("  link from {src}: {e}"),
            Ok((raw, c)) => {
                *cost.0 += 1; *cost.1 += c.bytes_received; *cost.2 += c.held_ms; *cost.3 += c.requests_sent;
                let before = out.len();
                for b in raw {
                    let Ok(h) = Header::scorex_parse_bytes(&b) else { break };
                    if !tachys_verify::tip::continues(&last, &h, epoch) { if out.is_empty() { off_at_start = true; } say!("  link from {src}: header at height {} does not follow the one before at the chain's difficulty", h.height); break }
                    if let Err(e) = header_ok(&h, anchor, now) { say!("  link from {src}: header at height {}: {e}", h.height); break }
                    last = h.clone(); out.push(h);
                }
                say!("  link from {src}: {} header(s), {} B, {} ms", out.len() - before, c.bytes_received, c.held_ms);
            }
        }
    }
    if out.len() < want { let n = out.len(); return Err((format!("{n} of the {want} headers between came, linked and checked"), out, off_at_start)) }
    if !tachys_verify::tip::continues(&last, to, epoch) { return Err(("the headers fetched do not lead to the agreed chain".into(), vec![], true)) }
    Ok(out)
}
