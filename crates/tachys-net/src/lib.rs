//! The smallest peer client that can ask a mainnet node for a NiPoPoW proof.
//! One connection, one handshake, one request, one answer; everything else the peer sends is ignored.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ergo_p2p::connection::Connection;
use ergo_p2p::framing::MAINNET_MAGIC as DEFAULT_MAGIC;

/// What every connection is made with: the network's magic, and what the handshake declares about this client.
#[derive(Clone, Copy)]
struct Wire {
    magic: [u8; 4],
    declared: Declared,
}
const WIRE: Wire = Wire {
    magic: DEFAULT_MAGIC,
    declared: Declared::Nothing,
};
/// The process's own, for a thread no `Own` has been entered on.
static SHARED: std::sync::RwLock<Wire> = std::sync::RwLock::new(WIRE);
/// A network and a declaration of one's own, for a program that talks to mainnet and to testnet at once: make one,
/// `enter` it on every thread the connections are made from (a runtime's `on_thread_start`), and what is set on those
/// threads is set for them alone. A thread that entered none shares the process's.
#[derive(Clone)]
pub struct Own(std::sync::Arc<std::sync::RwLock<Wire>>);
impl Default for Own {
    fn default() -> Self {
        Own(std::sync::Arc::new(std::sync::RwLock::new(WIRE)))
    }
}
thread_local! { static OWN: std::cell::RefCell<Option<Own>> = const { std::cell::RefCell::new(None) }; }
impl Own {
    /// From here on this thread's connections are made with this. Returns what the thread had, for `leave`.
    pub fn enter(&self) -> Option<Own> {
        OWN.with(|o| o.replace(Some(self.clone())))
    }
    /// Puts back what `enter` returned.
    pub fn leave(before: Option<Own>) {
        OWN.with(|o| *o.borrow_mut() = before);
    }
}
fn wire<R>(f: impl FnOnce(&mut Wire) -> R) -> R {
    match OWN.with(|o| o.borrow().clone()) {
        Some(own) => f(&mut own.0.write().unwrap_or_else(|p| p.into_inner())),
        None => f(&mut SHARED.write().unwrap_or_else(|p| p.into_inner())),
    }
}
/// Select the network for the connections that follow (default: mainnet). May be set again: one process can talk to
/// mainnet and then to testnet, and to both at once from threads that each entered an `Own`.
pub fn set_network_magic(m: [u8; 4]) {
    wire(|w| w.magic = m)
}
/// What the handshake declares about this client. See docs/good-neighbour.md.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Declared {
    /// No Mode feature at all (the default so far).
    Nothing,
    /// The truthful light declaration: digest state, not verifying transactions, NiPoPoW-bootstrapped
    /// (so not all headers), keeps no blocks.
    Light,
    /// A FALSE claim to be an archival node. For control arms on our own private network only.
    FullClaimForControlOnly,
}
pub fn set_declared(d: Declared) {
    wire(|w| w.declared = d)
}
fn mode_features() -> Vec<PeerFeature> {
    match wire(|w| w.declared) {
        Declared::Nothing => vec![],
        Declared::Light => vec![PeerFeature::Mode {
            state_type: 1,
            verify_tx: false,
            nipopow: Some(1),
            blocks_to_keep: 0,
        }],
        Declared::FullClaimForControlOnly => vec![PeerFeature::Mode {
            state_type: 0,
            verify_tx: true,
            nipopow: None,
            blocks_to_keep: -1,
        }],
    }
}
fn magic() -> [u8; 4] {
    wire(|w| w.magic)
}
use ergo_p2p::handshake::{
    deserialize_handshake_with_consumed, serialize_handshake, Handshake, PeerFeature, PeerSpec,
    Version,
};
use ergo_p2p::message::{
    deserialize_nipopow_proof, serialize_get_nipopow_proof, CODE_GET_NIPOPOW_PROOF,
    CODE_NIPOPOW_PROOF,
};
use ergo_p2p::types::NipopowProofData;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const AGENT: &str = "tachys";
pub const PROTOCOL: Version = Version {
    major: 6,
    minor: 0,
    patch: 8,
};
const MAX_HANDSHAKE: usize = 8096;

pub struct PeerProof {
    pub addr: String,
    pub agent: String,
    pub version: String,
    pub proof: Vec<u8>,
    /// REST API address the peer advertises in its handshake, if any.
    pub rest_url: Option<String>,
    pub connect_ms: u128,
    pub total_ms: u128,
    /// Frames the peer sent before the proof (inventory, sync info, peer requests).
    pub other_frames: usize,
    pub other_bytes: usize,
}

fn our_handshake() -> Handshake {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Handshake {
        time: now.as_millis() as u64,
        peer_spec: PeerSpec {
            agent_name: AGENT.to_string(),
            version: PROTOCOL,
            node_name: AGENT.to_string(),
            declared_address: None,
            features: mode_features()
                .into_iter()
                .chain(std::iter::once(PeerFeature::SessionId {
                    magic: magic(),
                    session_id: now.as_nanos() as i64,
                }))
                .collect(),
        },
    }
}

async fn handshake(stream: &mut TcpStream) -> Result<(Handshake, Vec<u8>), String> {
    stream
        .write_all(&serialize_handshake(&our_handshake()))
        .await
        .map_err(|e| format!("handshake write: {e}"))?;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("handshake read: {e}"))?;
        if n == 0 {
            return Err("closed during handshake".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Ok((hs, used)) =
            deserialize_handshake_with_consumed(&buf[..buf.len().min(MAX_HANDSHAKE)])
        {
            return Ok((hs, buf.split_off(used)));
        }
        if buf.len() > MAX_HANDSHAKE {
            return Err("oversized handshake".into());
        }
    }
}

/// Connect, shake hands, ask for a proof with parameters `m`, `k`, and wait up to `wait` for it.
pub async fn fetch_proof(addr: &str, m: i32, k: i32, wait: Duration) -> Result<PeerProof, String> {
    fetch_proof_padded(addr, m, k, &[], wait).await
}

/// As `fetch_proof`, with `pad` carried in the request's padding. The request ends with a length and that
/// many bytes "to allow adding new data in future"; a node that knows nothing of them reads and skips them.
pub async fn fetch_proof_padded(
    addr: &str,
    m: i32,
    k: i32,
    pad: &[u8],
    wait: Duration,
) -> Result<PeerProof, String> {
    if pad.len() > 127 {
        return Err("a pad of more than 127 bytes is not supported here".into());
    }
    let t0 = Instant::now();
    let mut stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| format!("connect: {e}"))?;
    let (hs, leftover) = tokio::time::timeout(Duration::from_secs(20), handshake(&mut stream))
        .await
        .map_err(|_| "handshake timeout".to_string())??;
    tokio::time::sleep(Duration::from_millis(250)).await; // see `open`: bytes sent with the handshake are discarded
    let connect_ms = t0.elapsed().as_millis();
    let v = hs.peer_spec.version;
    let rest_url = hs.peer_spec.features.iter().find_map(|f| match f {
        PeerFeature::RestApiUrl { url } => Some(url.clone()),
        _ => None,
    });
    let mut conn = Connection::new_with_buffer(stream, magic(), leftover);
    let mut request = serialize_get_nipopow_proof(&NipopowProofData {
        m,
        k,
        header_id_opt: None,
    });
    if !pad.is_empty() {
        request.pop(); // the serializer's own pad length, zero
        request.push(pad.len() as u8); // a length under 128 is one byte in this encoding
        request.extend_from_slice(pad);
    }
    conn.send(CODE_GET_NIPOPOW_PROOF, request)
        .await
        .map_err(|e| format!("request send: {e}"))?;
    let deadline = Instant::now() + wait;
    let (mut other_frames, mut other_bytes) = (0, 0);
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                format!(
                    "no proof within {} s ({} other frames)",
                    wait.as_secs(),
                    other_frames
                )
            })?;
        let frame = tokio::time::timeout(left, conn.read_message())
            .await
            .map_err(|_| {
                format!(
                    "no proof within {} s ({} other frames)",
                    wait.as_secs(),
                    other_frames
                )
            })?
            .map_err(|e| format!("read: {e}"))?;
        if frame.code == CODE_NIPOPOW_PROOF {
            let proof = deserialize_nipopow_proof(&frame.payload)
                .map_err(|e| format!("proof frame: {e}"))?;
            return Ok(PeerProof {
                addr: addr.to_string(),
                agent: hs.peer_spec.agent_name.clone(),
                version: format!("{}.{}.{}", v.major, v.minor, v.patch),
                proof,
                rest_url,
                connect_ms,
                total_ms: t0.elapsed().as_millis(),
                other_frames,
                other_bytes,
            });
        }
        other_frames += 1;
        other_bytes += frame.payload.len();
    }
}

// ---- tips: what peers push unprompted, and a short header chain from one peer ----

use ergo_p2p::message::{
    deserialize_inv, deserialize_modifiers, deserialize_sync_info, serialize_inv,
    serialize_sync_info, SyncInfo, CODE_INV, CODE_MODIFIER, CODE_REQUEST_MODIFIER, CODE_SYNC_INFO,
};
use ergo_p2p::types::InvData;

pub const HEADER_TYPE: u8 = 101;

/// What one connection cost the peer and us.
#[derive(Default, Clone, Debug)]
pub struct Cost {
    pub connect_ms: u128,
    pub held_ms: u128,
    pub requests_sent: usize,
    pub bytes_received: usize,
}

pub struct TipPush {
    pub addr: String,
    pub agent: String,
    pub version: String,
    /// Serialized headers from the peer's first sync message (tip first).
    pub headers: Vec<Vec<u8>>,
    pub first_sync_ms: u128,
    pub sync_frame_bytes: usize,
    /// True if the peer answered only after we sent our one sync message.
    pub provoked: bool,
    /// Header ids the peer announced as the continuation of the header we offered.
    pub extension_ids: usize,
    pub cost: Cost,
}

async fn open(addr: &str) -> Result<(Handshake, Connection, Instant, u128), String> {
    let t0 = Instant::now();
    let mut stream = tokio::time::timeout(Duration::from_secs(4), TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| format!("connect: {e}"))?;
    let (hs, leftover) = tokio::time::timeout(Duration::from_secs(20), handshake(&mut stream))
        .await
        .map_err(|_| "handshake timeout".to_string())??;
    // The reference node parses the handshake out of the first chunk it reads and discards whatever
    // else that chunk holds (PeerConnectionHandler.receiveAndHandleHandshake). A message sent right
    // behind our handshake can land in the same chunk and vanish, so give the node a moment first.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let ms = t0.elapsed().as_millis();
    Ok((
        hs,
        Connection::new_with_buffer(stream, magic(), leftover),
        t0,
        ms,
    ))
}

/// Connect and learn the peer's tip, holding the connection as briefly as possible.
///
/// Waits `quiet` for the peer's own sync message. If none comes and `behind` yields a header, sends
/// it in ONE sync message and waits `wait` more. Ban-safety rules (docs/good-neighbour.md): the
/// header sent must be genuine and behind the network tip, so the peer sees us as younger and
/// never asks us for anything; nothing else is ever sent.
pub async fn learn_tip(
    addr: &str,
    quiet: Duration,
    wait: Duration,
    mut behind: tokio::sync::watch::Receiver<Option<Vec<u8>>>,
) -> Result<TipPush, (String, Cost)> {
    let (hs, mut conn, t0, connect_ms) = open(addr).await.map_err(|e| (e, Cost::default()))?;
    let v = hs.peer_spec.version;
    let mut cost = Cost {
        connect_ms,
        ..Default::default()
    };
    let mut provoked = false;
    let mut extension_ids = 0usize;
    let mut deadline = Instant::now() + quiet;
    loop {
        cost.held_ms = t0.elapsed().as_millis();
        let left = match deadline.checked_duration_since(Instant::now()) {
            Some(l) => l,
            None if !provoked => {
                // quiet period over: provoke once if we have a header to offer
                let h = behind.borrow_and_update().clone();
                match h {
                    Some(bytes) => {
                        let payload = serialize_sync_info(&SyncInfo::V2 {
                            headers: vec![bytes],
                        })
                        .map_err(|e| (format!("sync encode: {e}"), cost.clone()))?;
                        conn.send(CODE_SYNC_INFO, payload)
                            .await
                            .map_err(|e| (format!("sync send: {e}"), cost.clone()))?;
                        cost.requests_sent += 1;
                        provoked = true;
                        deadline = Instant::now() + wait;
                        continue;
                    }
                    None => {
                        // nothing to offer yet: wait for another peer to supply one, bounded by `wait`
                        match tokio::time::timeout(wait, behind.changed()).await {
                            Ok(Ok(())) => continue,
                            _ => {
                                cost.held_ms = t0.elapsed().as_millis();
                                return Err(("silent, and no header to offer".into(), cost));
                            }
                        }
                    }
                }
            }
            None => {
                cost.held_ms = t0.elapsed().as_millis();
                return Err((
                    format!("no sync message {} ms after ours", wait.as_millis()),
                    cost,
                ));
            }
        };
        let frame = match tokio::time::timeout(left, conn.read_message()).await {
            Err(_) => continue, // loop back to the deadline handling
            Ok(Err(e)) => return Err((format!("read: {e}"), cost)),
            Ok(Ok(f)) => f,
        };
        cost.bytes_received += frame.payload.len() + 13;
        if frame.code == CODE_INV {
            if let Ok(inv) = deserialize_inv(&frame.payload) {
                if inv.type_id == HEADER_TYPE {
                    extension_ids += inv.ids.len();
                }
            }
        }
        if frame.code == CODE_SYNC_INFO {
            cost.held_ms = t0.elapsed().as_millis();
            return match deserialize_sync_info(&frame.payload) {
                Ok(SyncInfo::V2 { headers }) => Ok(TipPush {
                    addr: addr.to_string(),
                    agent: hs.peer_spec.agent_name.clone(),
                    version: format!("{}.{}.{}", v.major, v.minor, v.patch),
                    headers,
                    first_sync_ms: t0.elapsed().as_millis(),
                    sync_frame_bytes: frame.payload.len(),
                    provoked,
                    extension_ids,
                    cost,
                }),
                Ok(SyncInfo::V1 { .. }) => {
                    Err(("peer sent a version-1 sync message (ids only)".into(), cost))
                }
                Err(e) => Err((format!("sync message parse: {e}"), cost)),
            };
        }
    }
}

/// Ask one peer for headers by id, one request per header, following `next_id` from each answer.
/// `next_id` maps a received header's bytes to the id to ask for next (its parent), or None to stop.
pub async fn fetch_chain_back(
    addr: &str,
    start_id: [u8; 32],
    max: usize,
    wait: Duration,
    next_id: impl Fn(&[u8]) -> Option<[u8; 32]>,
) -> Result<(Vec<Vec<u8>>, Cost), String> {
    let (_hs, mut conn, t0, connect_ms) = open(addr).await?;
    let (mut out, mut bytes, mut requests, mut want) = (vec![], 0usize, 0usize, start_id);
    while out.len() < max {
        let payload = serialize_inv(&InvData {
            type_id: HEADER_TYPE,
            ids: vec![want],
        })
        .map_err(|e| format!("request encode: {e}"))?;
        conn.send(CODE_REQUEST_MODIFIER, payload)
            .await
            .map_err(|e| format!("request send: {e}"))?;
        requests += 1;
        let deadline = Instant::now() + wait;
        let got = loop {
            // a peer that does not hold the header simply does not answer: return what we have so far
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Ok((
                    out,
                    Cost {
                        connect_ms,
                        held_ms: t0.elapsed().as_millis(),
                        requests_sent: requests,
                        bytes_received: bytes,
                    },
                ));
            };
            let Ok(got) = tokio::time::timeout(left, conn.read_message()).await else {
                return Ok((
                    out,
                    Cost {
                        connect_ms,
                        held_ms: t0.elapsed().as_millis(),
                        requests_sent: requests,
                        bytes_received: bytes,
                    },
                ));
            };
            let frame = got.map_err(|e| format!("read: {e}"))?;
            bytes += frame.payload.len() + 13;
            if frame.code == CODE_MODIFIER {
                let data = deserialize_modifiers(&frame.payload)
                    .map_err(|e| format!("modifiers parse: {e}"))?;
                if data.type_id == HEADER_TYPE {
                    if let Some((_, b)) = data.modifiers.into_iter().find(|(id, _)| *id == want) {
                        break b;
                    }
                }
            }
        };
        let nxt = next_id(&got);
        out.push(got);
        match nxt {
            Some(id) => want = id,
            None => break,
        }
    }
    Ok((
        out,
        Cost {
            connect_ms,
            held_ms: t0.elapsed().as_millis(),
            requests_sent: requests,
            bytes_received: bytes,
        },
    ))
}

/// From a header the peer has, fetch the headers that follow it on the peer's chain, at most `max`: ONE sync
/// message offering `from` (a genuine header behind the tip: docs/good-neighbour.md), the ids the peer
/// announces in answer, and those headers by id. Returns them in the order announced. A longer run is fetched
/// by calling again, on a new connection, with the last header received.
///
/// A peer also announces each new block to everyone connected, in the same kind of message as its answer. So
/// an announcement is taken for the answer only once its first header is seen to be the child of the header
/// offered (`parent_of` reads a header's parent id); at most three announcements are asked for.
pub async fn fetch_run_forward(
    addr: &str,
    from: Vec<u8>,
    from_id: [u8; 32],
    max: usize,
    wait: Duration,
    parent_of: impl Fn(&[u8]) -> Option<[u8; 32]>,
) -> Result<(Vec<([u8; 32], Vec<u8>)>, Cost), String> {
    let (_hs, mut conn, t0, connect_ms) = open(addr).await?;
    let payload = serialize_sync_info(&SyncInfo::V2 {
        headers: vec![from],
    })
    .map_err(|e| format!("sync encode: {e}"))?;
    conn.send(CODE_SYNC_INFO, payload)
        .await
        .map_err(|e| format!("sync send: {e}"))?;
    let deadline = Instant::now() + wait;
    let (mut bytes, mut requests, mut asked) = (0usize, 1usize, 0usize);
    let mut batches: Vec<Vec<[u8; 32]>> = vec![];
    let mut got: std::collections::HashMap<[u8; 32], Vec<u8>> = std::collections::HashMap::new();
    let mut out = vec![];
    'read: while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(read) = tokio::time::timeout(left, conn.read_message()).await else {
            break;
        };
        let frame = read.map_err(|e| format!("read: {e}"))?;
        bytes += frame.payload.len() + 13;
        if frame.code == CODE_INV && asked < 3 {
            if let Ok(inv) = deserialize_inv(&frame.payload) {
                if inv.type_id == HEADER_TYPE && !inv.ids.is_empty() {
                    let ids: Vec<[u8; 32]> = inv.ids.into_iter().take(max).collect();
                    let request = serialize_inv(&InvData {
                        type_id: HEADER_TYPE,
                        ids: ids.clone(),
                    })
                    .map_err(|e| format!("request encode: {e}"))?;
                    conn.send(CODE_REQUEST_MODIFIER, request)
                        .await
                        .map_err(|e| format!("request send: {e}"))?;
                    requests += 1;
                    asked += 1;
                    batches.push(ids);
                }
            }
        } else if frame.code == CODE_MODIFIER {
            if let Ok(data) = deserialize_modifiers(&frame.payload) {
                if data.type_id == HEADER_TYPE {
                    got.extend(data.modifiers);
                }
            }
            for ids in &batches {
                // the answer to our sync message: complete, and its first header is the child of the one offered
                if ids.iter().all(|id| got.contains_key(id))
                    && parent_of(&got[&ids[0]]) == Some(from_id)
                {
                    out = ids.iter().map(|id| (*id, got[id].clone())).collect();
                    break 'read;
                }
            }
        }
    }
    Ok((
        out,
        Cost {
            connect_ms,
            held_ms: t0.elapsed().as_millis(),
            requests_sent: requests,
            bytes_received: bytes,
        },
    ))
}

pub const EXTENSION_TYPE: u8 = 108;

/// A block section as a peer serves it: its id and its bytes.
pub type Section = ([u8; 32], Vec<u8>);

/// Ask one peer for block sections of one type by id, in a single request. A peer leaves out what it does not
/// hold, so the answer may be shorter than the request, and is empty when `wait` runs out with nothing sent.
pub async fn fetch_sections(
    addr: &str,
    type_id: u8,
    ids: &[[u8; 32]],
    wait: Duration,
) -> Result<(Vec<Section>, Cost), String> {
    let (_hs, mut conn, t0, connect_ms) = open(addr).await?;
    let payload = serialize_inv(&InvData {
        type_id,
        ids: ids.to_vec(),
    })
    .map_err(|e| format!("request encode: {e}"))?;
    conn.send(CODE_REQUEST_MODIFIER, payload)
        .await
        .map_err(|e| format!("request send: {e}"))?;
    let deadline = Instant::now() + wait;
    let (mut out, mut bytes): (Vec<Section>, usize) = (vec![], 0);
    while out.len() < ids.len() {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let Ok(got) = tokio::time::timeout(left, conn.read_message()).await else {
            break;
        };
        let frame = got.map_err(|e| format!("read: {e}"))?;
        bytes += frame.payload.len() + 13;
        if frame.code == CODE_MODIFIER {
            let data = deserialize_modifiers(&frame.payload)
                .map_err(|e| format!("modifiers parse: {e}"))?;
            if data.type_id == type_id {
                out.extend(
                    data.modifiers
                        .into_iter()
                        .filter(|(id, _)| ids.contains(id)),
                );
            }
        }
    }
    Ok((
        out,
        Cost {
            connect_ms,
            held_ms: t0.elapsed().as_millis(),
            requests_sent: 1,
            bytes_received: bytes,
        },
    ))
}

/// Fetch headers forward from `from` (a genuine header's bytes, behind the peer's tip), as a syncing node does: offer
/// it in one sync message, take the header ids the peer announces as its continuation (a node announces up to 400),
/// request them in one message, then offer the newest received and go on, until `want` headers have come or the peer
/// announces nothing new. One sync message a second at most (a node takes one per 100 ms). Returns the headers
/// oldest first, each the child of the one before (by the ids `id_of` and `parent_of` read); nothing else about them
/// is checked here: the caller judges every header.
///
/// `budget` bounds the whole exchange: a peer that answers each round just before its deadline with a header or two
/// could otherwise hold a first run for hours. When it is spent, what came so far is returned and the caller goes on to
/// the next peer from there.
pub async fn sync_forward(
    addr: &str,
    from: Vec<u8>,
    want: usize,
    wait: Duration,
    budget: Duration,
    id_of: impl Fn(&[u8]) -> Option<[u8; 32]>,
    parent_of: impl Fn(&[u8]) -> Option<[u8; 32]>,
) -> Result<(Vec<Vec<u8>>, Cost), String> {
    let end = Instant::now() + budget;
    let (_hs, mut conn, t0, connect_ms) = open(addr).await?;
    let mut cost = Cost {
        connect_ms,
        ..Default::default()
    };
    let (mut out, mut last) = (Vec::<Vec<u8>>::new(), from);
    let mut last_sync: Option<Instant> = None;
    // a node also announces each new block as it comes, one id at a time: such an announcement is not the continuation
    // and its header does not link. Rounds that add nothing are allowed a few times before giving up
    let mut idle = 0;
    while out.len() < want && idle < 3 && Instant::now() < end {
        if let Some(t) = last_sync {
            if let Some(rest) = Duration::from_secs(1).checked_sub(t.elapsed()) {
                tokio::time::sleep(rest).await;
            }
        }
        let payload = serialize_sync_info(&SyncInfo::V2 {
            headers: vec![last.clone()],
        })
        .map_err(|e| format!("sync encode: {e}"))?;
        conn.send(CODE_SYNC_INFO, payload)
            .await
            .map_err(|e| format!("sync send: {e}"))?;
        cost.requests_sent += 1;
        last_sync = Some(Instant::now());
        // the continuation: an Inv of header ids (a peer may send its own sync message first; that is passed over)
        let deadline = (Instant::now() + wait).min(end);
        let ids: Vec<[u8; 32]> = loop {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break vec![];
            };
            let Ok(got) = tokio::time::timeout(left, conn.read_message()).await else {
                break vec![];
            };
            let frame = got.map_err(|e| format!("read: {e}"))?;
            cost.bytes_received += frame.payload.len() + 13;
            if frame.code == CODE_INV {
                if let Ok(inv) = deserialize_inv(&frame.payload) {
                    if inv.type_id == HEADER_TYPE && !inv.ids.is_empty() {
                        break inv.ids;
                    }
                }
            }
        };
        // only ids not had already, and no more than are wanted
        let had = id_of(&last);
        // each id once (a peer repeating one would otherwise keep the wait below from ever being met), and no more than are wanted
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<[u8; 32]> = ids
            .into_iter()
            .filter(|i| Some(*i) != had && seen.insert(*i))
            .take(want - out.len())
            .collect();
        if ids.is_empty() {
            break;
        }
        let payload = serialize_inv(&InvData {
            type_id: HEADER_TYPE,
            ids: ids.clone(),
        })
        .map_err(|e| format!("request encode: {e}"))?;
        conn.send(CODE_REQUEST_MODIFIER, payload)
            .await
            .map_err(|e| format!("request send: {e}"))?;
        cost.requests_sent += 1;
        let mut got: std::collections::HashMap<[u8; 32], Vec<u8>> =
            std::collections::HashMap::new();
        let deadline = (Instant::now() + wait).min(end);
        while got.len() < ids.len() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let Ok(r) = tokio::time::timeout(left, conn.read_message()).await else {
                break;
            };
            let frame = r.map_err(|e| format!("read: {e}"))?;
            cost.bytes_received += frame.payload.len() + 13;
            if frame.code != CODE_MODIFIER {
                continue;
            }
            let Ok(data) = deserialize_modifiers(&frame.payload) else {
                continue;
            };
            if data.type_id != HEADER_TYPE {
                continue;
            }
            for (id, b) in data.modifiers {
                if ids.contains(&id) {
                    got.insert(id, b);
                }
            }
        }
        // in the order announced, each the child of the one before, up to the first that is missing or is not: the rest
        // cannot be linked without it
        let before = out.len();
        for id in &ids {
            let Some(b) = got.remove(id) else { break };
            if parent_of(&b).is_none() || parent_of(&b) != id_of(&last) {
                if out.len() == before {
                    continue;
                } else {
                    break;
                }
            }
            last = b.clone();
            out.push(b);
        }
        if out.len() == before {
            idle += 1
        } else {
            idle = 0
        }
    }
    cost.held_ms = t0.elapsed().as_millis();
    Ok((out, cost))
}

// ---- box proofs over the peer protocol (prototype message; needs a node carrying our patch) ----

pub const CODE_GET_BOX_PROOF: u8 = 92;
pub const CODE_BOX_PROOF: u8 = 93;

pub struct BoxProof {
    /// Id of the block whose state root the proof is against, as named by the node.
    pub header_id: [u8; 32],
    pub proof: Vec<u8>,
    pub cost: Cost,
}

/// The most box ids one box-proof request may carry: what a node carrying the patch takes (`MaxIds` in
/// `node-patches/0001-box-proof-message.patch`). A caller with more asks in batches.
pub const MAX_BOX_PROOF_IDS: usize = 100;

/// Ask a peer for a batch proof of `ids` (1 to `MAX_BOX_PROOF_IDS`). One request, one answer.
pub async fn fetch_box_proof(
    addr: &str,
    ids: &[[u8; 32]],
    wait: Duration,
) -> Result<BoxProof, String> {
    // the count goes out as two bytes, and a node refuses more than its limit: refused here, before anything is sent
    if ids.is_empty() || ids.len() > MAX_BOX_PROOF_IDS {
        return Err(format!(
            "a box-proof request carries 1 to {MAX_BOX_PROOF_IDS} ids, not {}: ask in batches",
            ids.len()
        ));
    }
    use ergo_primitives::reader::VlqReader;
    use ergo_primitives::writer::VlqWriter;
    let (_hs, mut conn, t0, connect_ms) = open(addr).await?;
    let mut w = VlqWriter::new();
    w.put_u16(ids.len() as u16);
    for id in ids {
        w.put_bytes(id);
    }
    w.put_u16(0);
    conn.send(CODE_GET_BOX_PROOF, w.result())
        .await
        .map_err(|e| format!("request send: {e}"))?;
    let deadline = Instant::now() + wait;
    let mut bytes = 0usize;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("no box proof in time (does the peer carry the patch?)")?;
        let frame = tokio::time::timeout(left, conn.read_message())
            .await
            .map_err(|_| "no box proof in time (does the peer carry the patch?)".to_string())?
            .map_err(|e| format!("read: {e}"))?;
        bytes += frame.payload.len() + 13;
        if frame.code == CODE_BOX_PROOF {
            let mut r = VlqReader::new(&frame.payload);
            let mut header_id = [0u8; 32];
            header_id.copy_from_slice(
                r.get_bytes(32)
                    .map_err(|e| format!("box proof frame: {e}"))?,
            );
            let len = r
                .get_u32_exact()
                .map_err(|e| format!("box proof frame: {e}"))? as usize;
            let proof = r
                .get_bytes(len)
                .map_err(|e| format!("box proof frame: {e}"))?
                .to_vec();
            return Ok(BoxProof {
                header_id,
                proof,
                cost: Cost {
                    connect_ms,
                    held_ms: t0.elapsed().as_millis(),
                    requests_sent: 1,
                    bytes_received: bytes,
                },
            });
        }
    }
}

/// Connect and only watch: count the frames a peer sends us, by message code, for `secs` seconds.
/// Sends nothing after the handshake. Retries the connection for up to `retry` while the peer starts.
pub async fn observe(
    addr: &str,
    secs: u64,
    retry: Duration,
) -> Result<std::collections::BTreeMap<u8, (usize, usize)>, String> {
    let start = Instant::now();
    let (_hs, mut conn, _t0, _ms) = loop {
        match open(addr).await {
            Ok(x) => break x,
            Err(e) => {
                if start.elapsed() > retry {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    };
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut seen = std::collections::BTreeMap::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match tokio::time::timeout(left, conn.read_message()).await {
            Err(_) => break,
            Ok(Err(e)) => {
                seen.entry(255u8).or_insert((0usize, 0usize)).0 += 1;
                let _ = e;
                break;
            }
            Ok(Ok(f)) => {
                let e = seen.entry(f.code).or_insert((0usize, 0usize));
                e.0 += 1;
                e.1 += f.payload.len();
            }
        }
    }
    Ok(seen)
}

// ---- serving: a light peer that passes on the headers it has verified ----

#[derive(Default, Debug, Clone)]
pub struct ServeStats {
    pub reloads: usize,
    pub proofs_relayed: usize,
    pub parcels_taken: usize,
    pub connections: usize,
    pub sync_pushes: usize,
    pub header_requests: usize,
    pub headers_served: usize,
    pub bytes_sent: usize,
}

/// Listen on `listen` for `secs` seconds. To each peer that connects: push one sync message carrying
/// `headers` (id, bytes; newest first; the newest 50), then answer header requests for those ids, and a sync message
/// offering one of them with the ids that follow it. Nothing else is answered. At most `max_conns` peers at a time; each is dropped after 10 s idle.
pub type ParcelSink = std::sync::Arc<dyn Fn(&[u8]) -> Option<[u8; 32]> + Send + Sync>;

/// What a relaying wallet hands back for a box-proof request: the node's answer, plus any headers
/// newer than the ones it was already serving that the answer depends on (id, bytes; newest first).
pub struct RelayAnswer {
    pub header_id: [u8; 32],
    pub proof: Vec<u8>,
    pub new_headers: Vec<([u8; 32], Vec<u8>)>,
}
pub type ProofRelay = std::sync::Arc<
    dyn Fn(
            Vec<[u8; 32]>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<RelayAnswer>> + Send>>
        + Send
        + Sync,
>;

/// Called every few seconds while serving; returns a newer header list (id, bytes; newest first) when
/// the wallet has verified one since the last call.
pub type Reload = std::sync::Arc<dyn Fn() -> Option<Vec<([u8; 32], Vec<u8>)>> + Send + Sync>;

pub async fn serve(
    listen: &str,
    headers: Vec<([u8; 32], Vec<u8>)>,
    secs: u64,
    max_conns: usize,
    parcels: Option<ParcelSink>,
    relay: Option<ProofRelay>,
    reload: Option<Reload>,
) -> Result<ServeStats, String> {
    use ergo_p2p::message::serialize_modifiers;
    use ergo_p2p::types::ModifiersData;
    use std::sync::{Arc, Mutex};
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("bind {listen}: {e}"))?;
    let stats = Arc::new(Mutex::new(ServeStats::default()));
    let slots = Arc::new(tokio::sync::Semaphore::new(max_conns));
    let headers = Arc::new(Mutex::new(headers));
    let deadline = Instant::now() + Duration::from_secs(secs);
    if let Some(reload) = reload {
        let (headers, stats) = (headers.clone(), stats.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if Instant::now() >= deadline {
                    break;
                }
                if let Some(newer) = reload() {
                    *headers.lock().unwrap() = newer;
                    stats.lock().unwrap().reloads += 1;
                }
            }
        });
    }
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(Ok((mut stream, _peer))) = tokio::time::timeout(left, listener.accept()).await
        else {
            break;
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        }; // full: drop the connection
        let (stats, headers, parcels, relay) = (
            stats.clone(),
            headers.clone(),
            parcels.clone(),
            relay.clone(),
        );
        tokio::spawn(async move {
            let _slot = slot;
            let Ok(Ok((_hs, leftover))) =
                tokio::time::timeout(Duration::from_secs(10), handshake(&mut stream)).await
            else {
                return;
            };
            let mut conn = Connection::new_with_buffer(stream, magic(), leftover);
            stats.lock().unwrap().connections += 1;
            let push: Vec<Vec<u8>> = headers
                .lock()
                .unwrap()
                .iter()
                .take(50)
                .map(|h| h.1.clone())
                .collect();
            let Ok(payload) = serialize_sync_info(&SyncInfo::V2 { headers: push }) else {
                return;
            };
            let n = payload.len();
            if conn.send(CODE_SYNC_INFO, payload).await.is_err() {
                return;
            }
            {
                let mut s = stats.lock().unwrap();
                s.sync_pushes += 1;
                s.bytes_sent += n + 13;
            }
            // enough frames to pass on a long chain to a wallet syncing forward (two a round, 400 headers a round), and no
            // more than five minutes for one peer: slow peers must not hold every slot for the whole time served
            let until = Instant::now() + Duration::from_secs(300);
            for _ in 0..4096 {
                if Instant::now() >= until {
                    return;
                }
                let Ok(Ok(frame)) =
                    tokio::time::timeout(Duration::from_secs(10), conn.read_message()).await
                else {
                    return;
                };
                if frame.code == CODE_SYNC_INFO {
                    // a wallet syncing forward (`sync_forward`) offers a header it has: announce what follows it here,
                    // oldest first, up to 400, as a node does. A header we do not hold gets no answer.
                    let Ok(SyncInfo::V2 { headers: offered }) =
                        deserialize_sync_info(&frame.payload)
                    else {
                        continue;
                    };
                    let Some(theirs) = offered.last() else {
                        continue;
                    };
                    let next: Vec<[u8; 32]> = {
                        let hs = headers.lock().unwrap();
                        match hs.iter().position(|h| &h.1 == theirs) {
                            Some(i) => hs[..i].iter().rev().take(400).map(|h| h.0).collect(),
                            None => vec![],
                        }
                    };
                    if next.is_empty() {
                        continue;
                    }
                    let Ok(payload) = serialize_inv(&InvData {
                        type_id: HEADER_TYPE,
                        ids: next,
                    }) else {
                        return;
                    };
                    let n = payload.len();
                    if conn.send(CODE_INV, payload).await.is_err() {
                        return;
                    }
                    stats.lock().unwrap().bytes_sent += n + 13;
                    continue;
                }
                if frame.code == CODE_GET_BOX_PROOF {
                    // pass a box-proof request on to our own node and hand the answer back unchanged;
                    // the asker verifies it, so we add nothing to trust, only reach
                    if let Some(relay) = &relay {
                        let mut r = ergo_primitives::reader::VlqReader::new(&frame.payload);
                        let Ok(n) = r.get_u16() else { return };
                        if n == 0 || n as usize > MAX_BOX_PROOF_IDS {
                            continue;
                        }
                        let mut ids = Vec::with_capacity(n as usize);
                        for _ in 0..n {
                            let Ok(b) = r.get_bytes(32) else { return };
                            let mut id = [0u8; 32];
                            id.copy_from_slice(b);
                            ids.push(id);
                        }
                        if let Some(ans) = relay(ids).await {
                            {
                                let mut hs = headers.lock().unwrap();
                                let mut newer = ans.new_headers.clone();
                                newer.extend(hs.drain(..));
                                *hs = newer;
                            }
                            let mut w = ergo_primitives::writer::VlqWriter::new();
                            w.put_bytes(&ans.header_id);
                            w.put_u32(ans.proof.len() as u32);
                            w.put_bytes(&ans.proof);
                            w.put_u16(0);
                            let payload = w.result();
                            let n = payload.len();
                            if conn.send(CODE_BOX_PROOF, payload).await.is_err() {
                                return;
                            }
                            let mut s = stats.lock().unwrap();
                            s.proofs_relayed += 1;
                            s.bytes_sent += n + 13;
                        }
                    }
                    continue;
                }
                if frame.code == CODE_PARCEL {
                    // a signed payment handed over for later broadcast; the sink decides whether to keep it
                    if let Some(sink) = &parcels {
                        if frame.payload.len() <= MAX_PARCEL {
                            if let Some(id) = sink(&frame.payload) {
                                stats.lock().unwrap().parcels_taken += 1;
                                if conn.send(CODE_PARCEL_ACK, id.to_vec()).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    continue;
                }
                if frame.code != CODE_REQUEST_MODIFIER {
                    continue;
                }
                let Ok(inv) = deserialize_inv(&frame.payload) else {
                    return;
                };
                if inv.type_id != HEADER_TYPE {
                    continue;
                }
                let found: Vec<([u8; 32], Vec<u8>)> = {
                    let hs = headers.lock().unwrap();
                    let at: std::collections::HashMap<&[u8; 32], &Vec<u8>> =
                        hs.iter().map(|h| (&h.0, &h.1)).collect();
                    inv.ids
                        .iter()
                        .filter_map(|id| at.get(id).map(|b| (*id, (*b).clone())))
                        .collect()
                };
                {
                    let mut s = stats.lock().unwrap();
                    s.header_requests += 1;
                    s.headers_served += found.len();
                }
                if found.is_empty() {
                    continue;
                }
                let Ok(payload) = serialize_modifiers(&ModifiersData {
                    type_id: HEADER_TYPE,
                    modifiers: found,
                }) else {
                    return;
                };
                let n = payload.len();
                if conn.send(CODE_MODIFIER, payload).await.is_err() {
                    return;
                }
                stats.lock().unwrap().bytes_sent += n + 13;
            }
        });
    }
    let s = stats.lock().unwrap().clone();
    Ok(s)
}

// ---- store and forward: handing a signed payment to another wallet, and submitting one to a node ----

/// tachys-to-tachys only: a signed payment for the receiver to broadcast when it can.
pub const CODE_PARCEL: u8 = 120;
pub const CODE_PARCEL_ACK: u8 = 121;
pub const MAX_PARCEL: usize = 16 * 1024;
pub const TX_TYPE: u8 = 2;

/// Hand a parcel to a serving wallet. Returns the id it acknowledged.
pub async fn handoff(addr: &str, parcel: &[u8], wait: Duration) -> Result<[u8; 32], String> {
    if parcel.len() > MAX_PARCEL {
        return Err("parcel too large".into());
    }
    let (_hs, mut conn, _t0, _ms) = open(addr).await?;
    conn.send(CODE_PARCEL, parcel.to_vec())
        .await
        .map_err(|e| format!("parcel send: {e}"))?;
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("no acknowledgement (the peer may not accept parcels)")?;
        let frame = tokio::time::timeout(left, conn.read_message())
            .await
            .map_err(|_| "no acknowledgement (the peer may not accept parcels)".to_string())?
            .map_err(|e| format!("read: {e}"))?;
        if frame.code == CODE_PARCEL_ACK && frame.payload.len() == 32 {
            let mut id = [0u8; 32];
            id.copy_from_slice(&frame.payload);
            return Ok(id);
        }
    }
}

/// Submit a transaction to a node the way nodes do among themselves: announce its id, deliver the
/// bytes when the node asks. Returns false if the node never asked (it may already have it).
pub async fn submit_tx(
    addr: &str,
    tx_id: [u8; 32],
    tx_bytes: Vec<u8>,
    wait: Duration,
) -> Result<(bool, Cost), String> {
    use ergo_p2p::message::{serialize_modifiers, CODE_INV};
    use ergo_p2p::types::ModifiersData;
    let (_hs, mut conn, t0, connect_ms) = open(addr).await?;
    let inv = serialize_inv(&InvData {
        type_id: TX_TYPE,
        ids: vec![tx_id],
    })
    .map_err(|e| format!("inv encode: {e}"))?;
    conn.send(CODE_INV, inv)
        .await
        .map_err(|e| format!("inv send: {e}"))?;
    let deadline = Instant::now() + wait;
    let (mut bytes, mut asked) = (0usize, false);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(Ok(frame)) = tokio::time::timeout(left, conn.read_message()).await else {
            break;
        };
        bytes += frame.payload.len() + 13;
        if frame.code == CODE_REQUEST_MODIFIER {
            if let Ok(req) = deserialize_inv(&frame.payload) {
                if req.type_id == TX_TYPE && req.ids.contains(&tx_id) {
                    let payload = serialize_modifiers(&ModifiersData {
                        type_id: TX_TYPE,
                        modifiers: vec![(tx_id, tx_bytes.clone())],
                    })
                    .map_err(|e| format!("tx encode: {e}"))?;
                    conn.send(CODE_MODIFIER, payload)
                        .await
                        .map_err(|e| format!("tx send: {e}"))?;
                    asked = true;
                    tokio::time::sleep(Duration::from_millis(800)).await; // let it be read before we close
                    break;
                }
            }
        }
    }
    Ok((
        asked,
        Cost {
            connect_ms,
            held_ms: t0.elapsed().as_millis(),
            requests_sent: 1 + asked as usize,
            bytes_received: bytes,
        },
    ))
}

/// Fetch the headers from `from_id` back to (not including) `tip_id`, by id, newest first.
/// For a moment after a block is applied a node can name the block in a proof before it serves the
/// block's header, so an empty or short answer is retried a few times after a brief pause.
pub async fn fetch_linking(
    addr: &str,
    from_id: [u8; 32],
    tip_id: [u8; 32],
    max: usize,
    parent_of: impl Fn(&[u8]) -> Option<[u8; 32]> + Clone,
) -> Result<(Vec<Vec<u8>>, Cost), String> {
    let mut total = Cost::default();
    let mut last = vec![];
    for attempt in 0..4 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let p = parent_of.clone();
        let (raw, c) =
            fetch_chain_back(addr, from_id, max, Duration::from_millis(1500), move |b| {
                p(b).filter(|x| *x != tip_id)
            })
            .await?;
        total.requests_sent += c.requests_sent;
        total.bytes_received += c.bytes_received;
        total.held_ms += c.held_ms;
        let links = raw.last().and_then(|b| parent_of(b)) == Some(tip_id);
        last = raw;
        if links {
            break;
        }
    }
    Ok((last, total))
}
