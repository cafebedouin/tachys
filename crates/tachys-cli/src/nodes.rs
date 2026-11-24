//! Which nodes to ask for box proofs and to send through, when none is named (`--node auto`, the default for the
//! agent commands and the app).
//!
//! A few well-known nodes are asked which peers they know that publish a REST address; each candidate is then
//! looked at (`/info`, `/blockchain/indexedHeight`) and kept only if it is on this network, holds the UTXO set,
//! indexes boxes by address, and is at the chain's height. Those that fit are remembered in `<state>/nodes.json`; a
//! command is given the best few.
//!
//! None of this is trust. A node found this way is as untrusted as one named by hand: every box it shows is
//! proven against the chain the peers agreed on, and it can hide a box or drop a payment but not invent one.
//! What a found node does learn is what any node asked learns: the addresses looked up and the payments sent.

use std::net::{IpAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::chain::{load_chain, now_ms};
use crate::network::net;
use crate::util::{arg, clip, write_whole};

/// `--node auto`: find nodes instead of naming one.
pub(crate) const AUTO: &str = "auto";
/// How many nodes a command is given at most: the first that answers is used, the rest are fallbacks.
const USE: usize = 3;
/// A node advertised by a peer is anyone's. It is given to a command only to make the list up to this many: every
/// node on the list is asked which boxes the wallet's addresses hold, and so learns those addresses.
const FILL_TO: usize = 2;
/// A remembered list this recent is used without looking at the nodes again.
const KEEP_MS: u64 = 10 * 60 * 1000;
/// How far behind the chain's height a node may be.
const BEHIND: u64 = 3;
/// How far ahead of the stored chain a node may claim to be (the stored chain is a few minutes old at most when this is asked).
const AHEAD: u64 = 20;
const MAX_CANDIDATES: usize = 60;

/// The nodes asked first, and asked for the peers they know: long-standing public nodes (one run by an Ergo core
/// developer, the others at Cornell). They are where the search starts, not a promise: each is used only if it
/// answers and fits when asked.
pub(crate) fn well_known() -> &'static [&'static str] {
    match net().name {
        "mainnet" => &[
            "http://213.239.193.208:9053",
            "http://128.253.41.49:9053",
            "https://api.ergo.aap.cornell.edu",
            "http://api.ergo.aap.cornell.edu",
        ],
        "testnet" => &["http://213.239.193.208:9052"],
        _ => &[],
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub(crate) url: String,
    pub(crate) height: u64,
    pub(crate) ms: u64,
}

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            !(a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                || (a.octets()[0] == 100 && a.octets()[1] & 0xc0 == 64))
        }
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(m) => public(IpAddr::V4(m)),
            None => {
                !(a.is_loopback()
                    || a.is_unspecified()
                    || a.is_multicast()
                    || a.segments()[0] & 0xfe00 == 0xfc00
                    || a.segments()[0] & 0xffc0 == 0xfe80)
            }
        },
    }
}

/// A REST address a peer advertises is that peer's word. It is taken only as `http(s)://host[:port]`, nothing
/// after it, no user name in it: a shape that cannot carry a request of the peer's choosing.
pub(crate) fn tidy(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches('/');
    let rest = u
        .strip_prefix("http://")
        .or_else(|| u.strip_prefix("https://"))?;
    if u.len() > 200
        || rest.is_empty()
        || !rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
    {
        return None;
    }
    Some(u.to_string())
}
/// Does the address lead somewhere on the public internet? A peer may advertise `http://127.0.0.1:9053` or a
/// router's address; a wallet that followed it would be making requests inside its owner's own network.
fn leads_outside(url: &str) -> bool {
    let (default_port, rest) = match url.strip_prefix("https://") {
        Some(r) => (443, r),
        None => (80, url.strip_prefix("http://").unwrap_or(url)),
    };
    let with_port = if rest
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        rest.to_string()
    } else {
        format!("{rest}:{default_port}")
    };
    match with_port.to_socket_addrs() {
        Ok(a) => {
            let a: Vec<_> = a.collect();
            !a.is_empty() && a.iter().all(|s| public(s.ip()))
        }
        Err(_) => false,
    }
}

fn get(url: &str) -> Option<serde_json::Value> {
    use std::io::Read;
    let own = crate::own::own();
    let agent = own.http_quick.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .redirects(0)
            .build()
    });
    let mut text = String::new();
    agent
        .get(url)
        .call()
        .ok()?
        .into_reader()
        .take(2_000_000)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// What `/info` and `/blockchain/indexedHeight` must say for a node to be used: this network, the UTXO set, the
/// chain's height (`tip`, from the stored chain; 0 if there is none yet), and an index that has caught up.
pub(crate) fn fit(info: &serde_json::Value, indexed: &serde_json::Value, tip: u64) -> Option<u64> {
    let height = info["fullHeight"].as_u64()?;
    // at the chain's height: a few blocks behind is routine; far ahead of an agreed tip is a claim, not a height
    let ok = info["network"].as_str() == Some(net().name)
        && info["stateType"].as_str() == Some("utxo")
        && height + BEHIND >= tip
        && (tip == 0 || height <= tip + AHEAD)
        && indexed["indexedHeight"]
            .as_u64()
            .is_some_and(|i| i + BEHIND >= height);
    ok.then_some(height)
}

fn look_at(url: &str, tip: u64) -> Option<Found> {
    let t = Instant::now();
    let info = get(&format!("{url}/info"))?;
    let ms = t.elapsed().as_millis() as u64;
    let height = fit(
        &info,
        &get(&format!("{url}/blockchain/indexedHeight"))?,
        tip,
    )?;
    Some(Found {
        url: url.to_string(),
        height,
        ms,
    })
}

/// Every candidate at once, each on its own thread; the well-known nodes first, then by how fast they answered.
fn look_at_all(candidates: &[String], tip: u64) -> Vec<Found> {
    let mut found: Vec<Found> = std::thread::scope(|s| {
        let hs: Vec<_> = candidates
            .iter()
            .map(|u| {
                s.spawn(move || {
                    if leads_outside(u) {
                        look_at(u, tip)
                    } else {
                        None
                    }
                })
            })
            .collect();
        hs.into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });
    found.sort_by_key(|f| (!well_known().contains(&f.url.as_str()), f.ms));
    one_way_in(found)
}
/// A node that answered at both `http://HOST` and `https://HOST` is kept at the second only. Few nodes offer it, and
/// the plain one always answers sooner (no handshake), so by speed alone the wallet's addresses and payments went to
/// such a node in the clear though it would have taken them sealed.
fn one_way_in(found: Vec<Found>) -> Vec<Found> {
    let sealed: Vec<String> = found
        .iter()
        .filter_map(|f| f.url.strip_prefix("https://").map(str::to_string))
        .collect();
    found
        .into_iter()
        .filter(|f| {
            !f.url
                .strip_prefix("http://")
                .is_some_and(|host| sealed.iter().any(|s| s == host))
        })
        .collect()
}

/// The REST addresses the well-known nodes' peers advertise.
fn advertised() -> Vec<String> {
    let lists: Vec<serde_json::Value> = std::thread::scope(|s| {
        let hs: Vec<_> = well_known()
            .iter()
            .flat_map(|b| {
                ["peers/all", "peers/connected"].map(|p| s.spawn(move || get(&format!("{b}/{p}"))))
            })
            .collect();
        hs.into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });
    let mut out: Vec<String> = vec![];
    for u in lists
        .iter()
        .flat_map(|l| l.as_array().into_iter().flatten())
        .filter_map(|p| p["restApiUrl"].as_str().and_then(tidy))
    {
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

fn remembered(state: &str) -> (u64, Vec<String>) {
    let v: serde_json::Value = std::fs::read_to_string(format!("{state}/nodes.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null);
    if v["network"].as_str() != Some(net().name) {
        return (0, vec![]);
    }
    (
        v["at"].as_u64().unwrap_or(0),
        v["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n["url"].as_str().and_then(tidy))
            .collect(),
    )
}
fn remember(state: &str, found: &[Found]) {
    if state.is_empty() || !std::path::Path::new(state).is_dir() {
        return;
    }
    let v = serde_json::json!({ "at": now_ms(), "network": net().name, "nodes": found.iter().map(|f| serde_json::json!({ "url": f.url, "height": f.height, "ms": f.ms })).collect::<Vec<_>>() });
    let _ = write_whole(&format!("{state}/nodes.json"), v.to_string().as_bytes());
}

/// The nodes to use, best first. A recent remembered list is used as it is; an older one is looked at again and
/// kept if two of its nodes still fit; otherwise the well-known nodes are asked for candidates and all are looked at.
pub(crate) fn find(state: &str, refresh: bool) -> Result<Vec<Found>, String> {
    if well_known().is_empty() {
        return Err("a private network has no nodes to find: give --node URL".into());
    }
    let tip = load_chain(state).first().map_or(0, |h| h.height as u64);
    let (at, known) = remembered(state);
    if !refresh && !known.is_empty() {
        if now_ms().saturating_sub(at) < KEEP_MS {
            return Ok(known
                .into_iter()
                .map(|url| Found {
                    url,
                    height: 0,
                    ms: 0,
                })
                .collect());
        }
        let still = look_at_all(&known, tip);
        if still.len() >= 2 {
            remember(state, &still);
            return Ok(still);
        }
    }
    let mut candidates: Vec<String> = well_known().iter().map(|s| s.to_string()).collect();
    for u in known.into_iter().chain(advertised()) {
        if !candidates.contains(&u) && candidates.len() < MAX_CANDIDATES {
            candidates.push(u);
        }
    }
    let found = look_at_all(&candidates, tip);
    if found.is_empty() {
        return Err(format!("no usable node found among {} candidates (none reachable, or none at the chain's height with the UTXO set and a box index); name one with --node URL", candidates.len()));
    }
    remember(state, &found);
    Ok(found)
}

/// What a command was given for `--node`: as it is, unless it is `auto`, which becomes the best few found nodes
/// (comma-separated, as a list given by hand would be).
pub(crate) fn resolve(node: &str, state: &str) -> Result<String, String> {
    if node != AUTO {
        return Ok(node.trim_end_matches('/').to_string());
    }
    Ok(chosen(&find(state, false)?).join(","))
}
/// The nodes a command is given, from those found (best first): the well-known ones, and advertised ones only
/// while there are fewer than `FILL_TO`.
pub(crate) fn chosen(found: &[Found]) -> Vec<String> {
    let known = |f: &&Found| well_known().contains(&f.url.as_str());
    let mut out: Vec<String> = found
        .iter()
        .filter(known)
        .take(USE)
        .map(|f| f.url.clone())
        .collect();
    for f in found.iter().filter(|f| !known(f)) {
        if out.len() >= FILL_TO {
            break;
        }
        out.push(f.url.clone());
    }
    out
}
/// `resolve`, for a command: with no node to be found it ends, refused.
pub(crate) fn resolved(node: &str, state: &str) -> Result<String, crate::out::Fail> {
    resolve(node, state).map_err(|e| crate::out::Fail::new(1, format!("no node: {e}")))
}
/// The nodes of a `--node` value, in order.
pub(crate) fn each(nodes: &str) -> Vec<String> {
    nodes
        .split(',')
        .map(|n| n.trim().trim_end_matches('/').to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// tachys nodes [--state DIR] [--refresh]: find usable nodes and say which they are.
pub(crate) fn nodes(args: &[String]) -> crate::out::Ended {
    let state: String = arg(args, "--state", "state".to_string())?;
    match find(&state, args.iter().any(|a| a == "--refresh")) {
        Err(e) => {
            say!("{e}");
            return Err(crate::out::Fail::said(1));
        }
        Ok(found) => {
            let used = chosen(&found);
            crate::out::set("nodes", found.iter().map(|f| serde_json::json!({ "url": f.url, "height": f.height, "ms": f.ms, "well_known": well_known().contains(&f.url.as_str()), "used": used.contains(&f.url) })).collect::<Vec<_>>());
            for f in found.iter() {
                say!(
                    "node {}  {}{}{}",
                    clip(&f.url, 200),
                    if f.height > 0 {
                        format!("height {}, answered in {} ms", f.height, f.ms)
                    } else {
                        "remembered".to_string()
                    },
                    if well_known().contains(&f.url.as_str()) {
                        ", well-known"
                    } else {
                        ", advertised by a peer"
                    },
                    if used.contains(&f.url) {
                        "  (used)"
                    } else {
                        ""
                    }
                );
            }
            say!("{} usable node(s); a command given `--node auto` uses the {} marked, in this order", found.len(), used.len());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_that_answers_both_ways_is_asked_over_https_only() {
        let f = |url: &str, ms: u64| Found {
            url: url.into(),
            height: 1,
            ms,
        };
        // the plain one answered sooner, as it always will, and is first on the list by speed
        let found = vec![
            f("http://api.example.org", 40),
            f("http://203.0.113.9:9053", 60),
            f("https://api.example.org", 180),
        ];
        assert_eq!(
            one_way_in(found)
                .iter()
                .map(|f| f.url.as_str())
                .collect::<Vec<_>>(),
            ["http://203.0.113.9:9053", "https://api.example.org"]
        );
        // a node with no https is kept as it is, and so is one whose https is at another port
        let found = vec![
            f("http://203.0.113.9:9053", 60),
            f("https://203.0.113.9", 90),
        ];
        assert_eq!(one_way_in(found).len(), 2);
    }

    #[test]
    fn an_advertised_address_is_taken_only_in_one_shape() {
        assert_eq!(
            tidy(" http://128.253.41.49:9053/ "),
            Some("http://128.253.41.49:9053".into())
        );
        assert_eq!(
            tidy("https://node.example.org"),
            Some("https://node.example.org".into())
        );
        assert_eq!(
            tidy("http://[2001:db8::1]:9053"),
            Some("http://[2001:db8::1]:9053".into())
        );
        for bad in [
            "",
            "http://",
            "ftp://node.example.org",
            "node.example.org:9053",
            "http://user@node.example.org",
            "http://node.example.org/path",
            "http://node.example.org?x=1",
            "http://node.example.org#x",
            "http://a b",
            "http://a\nbroadcast via x: accepted, y",
            "file:///etc/passwd",
        ] {
            assert_eq!(tidy(bad), None, "{bad:?}");
        }
        assert_eq!(
            tidy(&format!("http://{}", "a".repeat(200))),
            None,
            "too long"
        );
    }

    #[test]
    fn an_address_inside_the_owners_own_network_is_not_followed() {
        for inside in [
            "http://127.0.0.1:9053",
            "http://localhost:9053",
            "http://10.0.0.5:9053",
            "http://192.168.1.1",
            "http://172.16.0.9:9053",
            "http://169.254.1.1:9053",
            "http://100.64.0.1:9053",
            "http://0.0.0.0:9053",
            "http://[::1]:9053",
            "http://[fe80::1]:9053",
            "http://[fd00::1]:9053",
            "http://[::ffff:10.0.0.5]:9053",
        ] {
            assert!(!leads_outside(inside), "{inside}");
        }
        assert!(leads_outside("http://128.253.41.49:9053"));
        assert!(leads_outside("https://128.253.41.49"));
        assert!(leads_outside("http://[2001:4860:4860::8888]:9053"));
    }

    #[test]
    fn a_node_is_used_only_if_it_can_do_what_the_wallet_asks() {
        let info = |network: &str, state: &str, height: u64| serde_json::json!({ "network": network, "stateType": state, "fullHeight": height });
        let index = |h: u64| serde_json::json!({ "indexedHeight": h, "fullHeight": h });
        assert_eq!(
            fit(&info("mainnet", "utxo", 1000), &index(1000), 1000),
            Some(1000)
        );
        assert_eq!(
            fit(&info("mainnet", "utxo", 998), &index(996), 1000),
            Some(998),
            "a few blocks behind is routine"
        );
        assert_eq!(
            fit(&info("mainnet", "utxo", 2000), &index(2000), 0),
            Some(2000),
            "no chain stored yet: any height"
        );
        assert_eq!(
            fit(&info("testnet", "utxo", 1000), &index(1000), 1000),
            None,
            "another network"
        );
        assert_eq!(
            fit(&info("mainnet", "digest", 1000), &index(1000), 1000),
            None,
            "no UTXO set: it cannot prove a box"
        );
        assert_eq!(
            fit(&info("mainnet", "utxo", 900), &index(900), 1000),
            None,
            "well behind the chain"
        );
        assert_eq!(
            fit(&info("mainnet", "utxo", 1000), &index(500), 1000),
            None,
            "its index has not caught up"
        );
        assert_eq!(
            fit(&info("mainnet", "utxo", 9_000_000), &index(9_000_000), 1000),
            None,
            "a height far ahead of the chain is a claim"
        );
        assert_eq!(
            fit(&info("mainnet", "utxo", 1010), &index(1010), 1000),
            Some(1010),
            "a few blocks ahead of a tip agreed minutes ago is routine"
        );
        assert_eq!(
            fit(
                &info("mainnet", "utxo", 1000),
                &serde_json::json!({ "error": 404 }),
                1000
            ),
            None,
            "no index"
        );
        assert_eq!(
            fit(
                &serde_json::json!({ "network": "mainnet", "stateType": "utxo", "fullHeight": null }),
                &index(1000),
                1000
            ),
            None,
            "still syncing"
        );
    }

    #[test]
    fn a_peers_node_is_used_only_when_the_well_known_ones_are_too_few() {
        let f = |url: &str| Found {
            url: url.into(),
            height: 1,
            ms: 1,
        };
        let (k1, k2, k3) = (well_known()[0], well_known()[1], well_known()[2]);
        assert_eq!(
            chosen(&[f("https://a.example"), f(k1), f(k2), f("https://b.example")]),
            vec![k1, k2],
            "two well-known nodes are enough"
        );
        assert_eq!(
            chosen(&[f(k1), f(k2), f(k3), f(well_known()[3])]),
            vec![k1, k2, k3],
            "never more than three"
        );
        assert_eq!(
            chosen(&[f("https://a.example"), f(k2), f("https://b.example")]),
            vec![k2, "https://a.example"],
            "one well-known node: one more, for a fallback"
        );
        assert_eq!(
            chosen(&[
                f("https://a.example"),
                f("https://b.example"),
                f("https://c.example")
            ]),
            vec!["https://a.example", "https://b.example"]
        );
        assert!(chosen(&[]).is_empty());
    }

    #[test]
    fn a_list_of_nodes_is_read_in_order() {
        assert_eq!(
            each("http://a:1/, http://b:2 ,,p2p://c:3"),
            vec!["http://a:1", "http://b:2", "p2p://c:3"]
        );
        assert!(each("").is_empty());
    }
}
