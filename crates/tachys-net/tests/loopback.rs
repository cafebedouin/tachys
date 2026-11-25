//! The peer client against peers on this machine: our own `serve` for what an honest peer does, and a bare
//! listener for what a hostile or broken one does. Nothing here leaves the loopback interface.
//! Header bytes are opaque to this crate, so the "headers" are 40 bytes: the parent's id, then filler.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ergo_p2p::connection::Connection;
use ergo_p2p::framing::MAINNET_MAGIC;
use ergo_p2p::handshake::{serialize_handshake, Handshake, PeerSpec};
use tachys_net::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SHORT: Duration = Duration::from_millis(1500);

fn id(i: u8) -> [u8; 32] {
    [i; 32]
}
/// Header `i`: its parent is header `i - 1`.
fn header(i: u8) -> ([u8; 32], Vec<u8>) {
    let mut b = id(i - 1).to_vec();
    b.extend([0xEE; 8]);
    (id(i), b)
}
fn parent(b: &[u8]) -> Option<[u8; 32]> {
    b.get(..32).and_then(|p| <[u8; 32]>::try_from(p).ok())
}
/// Headers 9, 8, ... 2 (newest first), as a wallet serves them.
fn chain() -> Vec<([u8; 32], Vec<u8>)> {
    (2..=9).rev().map(header).collect()
}

/// A port nothing is listening on yet.
async fn free_addr() -> String {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string()
}

/// Our own serving wallet on a fresh port, for a few seconds.
async fn serving(parcels: Option<ParcelSink>, relay: Option<ProofRelay>) -> String {
    let addr = free_addr().await;
    let a = addr.clone();
    tokio::spawn(async move {
        let _ = serve(&a, chain(), 8, 8, parcels, relay, None).await;
    });
    // until it listens: a fixed pause lost the race on a loaded machine ("connection refused")
    let t = Instant::now();
    while tokio::net::TcpStream::connect(&addr).await.is_err() {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "the serving wallet did not start listening"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    addr
}
fn nothing_to_offer() -> tokio::sync::watch::Receiver<Option<Vec<u8>>> {
    tokio::sync::watch::channel(None).1
}

#[tokio::test(flavor = "multi_thread")]
async fn a_serving_wallet_pushes_its_headers_and_a_client_hears_them() {
    let addr = serving(None, None).await;
    let tip = learn_tip(&addr, SHORT, SHORT, nothing_to_offer())
        .await
        .map_err(|e| e.0)
        .unwrap();
    assert_eq!(
        tip.headers,
        chain().into_iter().map(|h| h.1).collect::<Vec<_>>(),
        "every header, newest first, byte for byte"
    );
    assert!(!tip.provoked, "pushed unasked: the client sent nothing");
    assert_eq!(tip.cost.requests_sent, 0);
    assert_eq!(tip.agent, AGENT);
}

#[tokio::test(flavor = "multi_thread")]
async fn headers_are_fetched_back_by_id_one_parent_at_a_time() {
    let addr = serving(None, None).await;
    // from header 8 back: three are asked for, three come, oldest last
    let (got, cost) = fetch_chain_back(&addr, id(8), 3, SHORT, parent)
        .await
        .unwrap();
    assert_eq!(got, vec![header(8).1, header(7).1, header(6).1]);
    assert_eq!(cost.requests_sent, 3);
    // the peer holds nothing below header 2: what it had is returned, and the missing one is not waited on for ever
    let t = Instant::now();
    let (got, _) = fetch_chain_back(&addr, id(3), 5, Duration::from_millis(600), parent)
        .await
        .unwrap();
    assert_eq!(got, vec![header(3).1, header(2).1]);
    assert!(t.elapsed() < Duration::from_secs(4));
    // a header it never had: nothing, not an error
    let (got, _) = fetch_chain_back(&addr, id(77), 2, Duration::from_millis(400), parent)
        .await
        .unwrap();
    assert!(got.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn headers_between_a_named_block_and_the_tip_are_fetched_until_they_link() {
    let addr = serving(None, None).await;
    let (got, _) = fetch_linking(&addr, id(9), id(6), 12, parent)
        .await
        .unwrap();
    assert_eq!(
        got,
        vec![header(9).1, header(8).1, header(7).1],
        "down to the header whose parent is the tip, and no further"
    );
    assert_eq!(parent(got.last().unwrap()), Some(id(6)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parcel_is_acknowledged_only_by_a_wallet_that_takes_them() {
    let taken: Arc<Mutex<Vec<Vec<u8>>>> = Default::default();
    let t = taken.clone();
    let sink: ParcelSink = Arc::new(move |raw: &[u8]| {
        if raw.starts_with(b"no") {
            return None;
        }
        t.lock().unwrap().push(raw.to_vec());
        Some(id(42))
    });
    let addr = serving(Some(sink), None).await;
    assert_eq!(handoff(&addr, b"a signed payment", SHORT).await, Ok(id(42)));
    assert_eq!(*taken.lock().unwrap(), vec![b"a signed payment".to_vec()]);
    // the sink turned it down: no acknowledgement, and the sender is told so
    assert!(handoff(&addr, b"no thanks", Duration::from_millis(500))
        .await
        .unwrap_err()
        .contains("no acknowledgement"));
    // too large to send at all
    assert_eq!(
        handoff(&addr, &vec![0u8; MAX_PARCEL + 1], SHORT).await,
        Err("parcel too large".to_string())
    );
    // a wallet that serves headers only says nothing to a parcel
    let plain = serving(None, None).await;
    assert!(
        handoff(&plain, b"a signed payment", Duration::from_millis(500))
            .await
            .unwrap_err()
            .contains("no acknowledgement")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_proof_request_is_relayed_and_the_headers_it_rests_on_are_served() {
    let asked: Arc<Mutex<Vec<Vec<[u8; 32]>>>> = Default::default();
    let a = asked.clone();
    let relay: ProofRelay = Arc::new(move |ids: Vec<[u8; 32]>| {
        a.lock().unwrap().push(ids);
        Box::pin(async move {
            Some(RelayAnswer {
                header_id: id(10),
                proof: vec![7, 7, 7, 7, 7],
                new_headers: vec![header(10)],
            })
        })
    });
    let addr = serving(None, Some(relay)).await;
    let bp = fetch_box_proof(&addr, &[id(1), id(2)], SHORT)
        .await
        .unwrap();
    assert_eq!(
        (bp.header_id, bp.proof.as_slice()),
        (id(10), &[7u8, 7, 7, 7, 7][..])
    );
    assert_eq!(
        *asked.lock().unwrap(),
        vec![vec![id(1), id(2)]],
        "the ids asked reach the relay as sent"
    );
    // the block the proof names was newer than what was served; it is served now
    let (got, _) = fetch_chain_back(&addr, id(10), 1, SHORT, parent)
        .await
        .unwrap();
    assert_eq!(got, vec![header(10).1]);
    // a full batch, as many ids as a node takes in one request, is relayed too
    let full: Vec<[u8; 32]> = (0..MAX_BOX_PROOF_IDS as u8).map(id).collect();
    assert!(
        fetch_box_proof(&addr, &full, SHORT).await.is_ok(),
        "a batch of {MAX_BOX_PROOF_IDS} ids is relayed"
    );
    assert_eq!(
        asked.lock().unwrap().last().unwrap().len(),
        MAX_BOX_PROOF_IDS
    );
    // a wallet that does not relay never answers, and the client gives up in time
    let plain = serving(None, None).await;
    let t = Instant::now();
    assert!(
        fetch_box_proof(&plain, &[id(1)], Duration::from_millis(500))
            .await
            .err()
            .unwrap()
            .contains("no box proof in time")
    );
    assert!(t.elapsed() < Duration::from_secs(3));
}

// ---- peers that misbehave ----

fn their_handshake() -> Vec<u8> {
    serialize_handshake(&Handshake {
        time: 1,
        peer_spec: PeerSpec {
            agent_name: "other".into(),
            // (this peer speaks through the vendored codec, which has a version type of its own)
            version: ergo_p2p::handshake::Version {
                major: PROTOCOL.major,
                minor: PROTOCOL.minor,
                patch: PROTOCOL.patch,
            },
            node_name: "other".into(),
            declared_address: None,
            features: vec![],
        },
    })
}
/// A peer that completes the handshake and then does `then` with the connection.
async fn peer<F, Fut>(then: F) -> String
where
    F: FnOnce(Connection) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = s.read(&mut buf).await;
        s.write_all(&their_handshake()).await.unwrap();
        then(Connection::new_with_buffer(s, MAINNET_MAGIC, vec![])).await;
    });
    addr
}
/// A peer that answers the connection with these raw bytes and closes.
async fn raw_peer(bytes: Vec<u8>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let _ = s.write_all(&bytes).await;
        let _ = s.shutdown().await;
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_is_not_there_or_does_not_shake_hands_is_an_error_not_a_hang() {
    let t = Instant::now();
    let nobody = free_addr().await;
    assert!(learn_tip(&nobody, SHORT, SHORT, nothing_to_offer())
        .await
        .err()
        .unwrap()
        .0
        .contains("connect"));
    assert!(fetch_chain_back(&nobody, id(1), 1, SHORT, parent)
        .await
        .unwrap_err()
        .contains("connect"));
    // closes without a word
    assert_eq!(
        fetch_box_proof(&raw_peer(vec![]).await, &[id(1)], SHORT)
            .await
            .err()
            .unwrap(),
        "closed during handshake"
    );
    // talks, but not the protocol: far more than a handshake may be
    let e = handoff(&raw_peer(vec![0xFF; 20_000]).await, b"x", SHORT)
        .await
        .unwrap_err();
    assert!(e.contains("handshake"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn frames_that_are_not_what_was_asked_for_are_refused() {
    // another network's frames after a good handshake
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = s.read(&mut buf).await;
        s.write_all(&their_handshake()).await.unwrap();
        let mut c = Connection::new_with_buffer(s, [9, 9, 9, 9], vec![]);
        let _ = c.send(65, vec![1, 2, 3]).await;
        tokio::time::sleep(SHORT).await;
    });
    let e = learn_tip(&addr, SHORT, SHORT, nothing_to_offer())
        .await
        .err()
        .unwrap()
        .0;
    assert!(e.starts_with("read: "), "{e}");
    // a sync message that does not parse
    let addr = peer(|mut c| async move {
        let _ = c.send(65, vec![0xFF; 9]).await;
        tokio::time::sleep(SHORT).await;
    })
    .await;
    let e = learn_tip(&addr, SHORT, SHORT, nothing_to_offer())
        .await
        .err()
        .unwrap()
        .0;
    assert!(e.starts_with("sync message parse"), "{e}");
    // a box proof cut short: the length says 200 bytes and two follow
    let addr = peer(|mut c| async move {
        let mut p = id(5).to_vec();
        p.extend([0xC8, 0x01, 9, 9]);
        let _ = c.send(CODE_BOX_PROOF, p).await;
        tokio::time::sleep(SHORT).await;
    })
    .await;
    assert!(fetch_box_proof(&addr, &[id(1)], SHORT)
        .await
        .err()
        .unwrap()
        .contains("box proof frame"));
    // and one shorter than a block id
    let addr = peer(|mut c| async move {
        let _ = c.send(CODE_BOX_PROOF, vec![1, 2, 3]).await;
        tokio::time::sleep(SHORT).await;
    })
    .await;
    assert!(fetch_box_proof(&addr, &[id(1)], SHORT)
        .await
        .err()
        .unwrap()
        .contains("box proof frame"));
    // a header answer for an id that was not asked: ignored, and the request times out with nothing taken
    let addr = peer(|mut c| async move {
        let wrong = ergo_p2p::message::serialize_modifiers(&ergo_p2p::types::ModifiersData {
            type_id: HEADER_TYPE,
            modifiers: vec![header(50)],
        })
        .unwrap();
        let _ = c.read_message().await;
        let _ = c.send(33, wrong).await;
        tokio::time::sleep(SHORT).await;
    })
    .await;
    let (got, _) = fetch_chain_back(&addr, id(8), 2, Duration::from_millis(500), parent)
        .await
        .unwrap();
    assert!(
        got.is_empty(),
        "a header nobody asked for is not taken in place of the one that was"
    );
    // an acknowledgement of the wrong length is not an acknowledgement
    let addr = peer(|mut c| async move {
        let _ = c.read_message().await;
        let _ = c.send(CODE_PARCEL_ACK, vec![1; 31]).await;
        tokio::time::sleep(SHORT).await;
    })
    .await;
    assert!(handoff(&addr, b"x", Duration::from_millis(500))
        .await
        .unwrap_err()
        .contains("no acknowledgement"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_peer_is_offered_one_header_and_then_left() {
    // says nothing until it is sent a sync message, then answers with its own
    let addr = peer(|mut c| async move {
        let asked = c.read_message().await.unwrap();
        assert_eq!(asked.code, 65);
        let push = ergo_p2p::message::serialize_sync_info(&ergo_p2p::message::SyncInfo::V2 {
            headers: vec![header(9).1],
        })
        .unwrap();
        let _ = c.send(65, push).await;
        tokio::time::sleep(SHORT).await;
    })
    .await;
    let (_tx, rx) = tokio::sync::watch::channel(Some(header(3).1));
    let tip = learn_tip(&addr, Duration::from_millis(300), SHORT, rx)
        .await
        .map_err(|e| e.0)
        .unwrap();
    assert!(tip.provoked);
    assert_eq!(
        (tip.headers, tip.cost.requests_sent),
        (vec![header(9).1], 1),
        "one message sent, no more"
    );
    // silent, and nothing to offer it: given up on within the wait
    let addr = peer(|c| async move {
        let _held = c;
        tokio::time::sleep(Duration::from_secs(3)).await;
    })
    .await;
    let t = Instant::now();
    assert!(learn_tip(
        &addr,
        Duration::from_millis(200),
        Duration::from_millis(400),
        nothing_to_offer()
    )
    .await
    .err()
    .unwrap()
    .0
    .contains("no header to offer"));
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_proof_request_carries_one_to_a_hundred_ids_or_is_not_sent() {
    // the count goes out as two bytes and a node carrying the patch takes at most 100: more is refused before anything is
    // sent (no peer listens at this address, and the answer says why, not that it could not connect)
    let nowhere = free_addr().await;
    for n in [0usize, MAX_BOX_PROOF_IDS + 1, 70_000] {
        let ids = vec![[1u8; 32]; n];
        let e = fetch_box_proof(&nowhere, &ids, SHORT).await.err().unwrap();
        assert!(e.contains("1 to 100 ids"), "{n} ids: {e}");
    }
    let e = fetch_box_proof(&nowhere, &[[1u8; 32]; MAX_BOX_PROOF_IDS], SHORT)
        .await
        .err()
        .unwrap();
    assert!(
        !e.contains("1 to 100 ids"),
        "a hundred are sent (and here find no peer): {e}"
    );
}
