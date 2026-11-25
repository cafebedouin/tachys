//! The peer protocol's bytes, for the few messages this client speaks: whole numbers as the reference node writes
//! them, a message's frame, the handshake, and inventory, modifiers, sync info and the two NiPoPoW messages.
//!
//! This is the wallet's own codec. It took the place of a vendored one (`vendor/arkadianet-codec`, a full node's
//! peer stack) of which the wallet used this much and no more. The vendored one is kept for the tests, which hold
//! every encoder and decoder here to it, both ways, on made-up and on random bytes (`tests/wire.rs`); the names are
//! the vendored one's, so that the two can be read side by side.
//!
//! Every length here is a peer's word. Nothing is allocated on one before the bytes it promises are in hand, and
//! nothing indexes past what was received: a read that would is `Malformed`.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Bytes that are not what they are said to be, and what was being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed(pub &'static str);
impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Malformed {}

// ---- whole numbers ----

/// Reads the reference node's numbers from bytes a peer sent: VLQ (seven bits a byte, the low ones first, the top
/// bit saying another byte follows) and ZigZag over it for the signed ones.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    /// How many bytes have been read.
    pub fn position(&self) -> usize {
        self.pos
    }
    /// How many are left.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn get_u8(&mut self) -> Result<u8, Malformed> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or(Malformed("the bytes end where a byte is expected"))?;
        self.pos += 1;
        Ok(b)
    }
    pub fn get_bytes(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        if n > self.remaining() {
            return Err(Malformed("the bytes end before the length they give"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }
    pub fn get_id(&mut self) -> Result<[u8; 32], Malformed> {
        let mut id = [0u8; 32];
        id.copy_from_slice(self.get_bytes(32)?);
        Ok(id)
    }
    /// A VLQ number of up to 64 bits. Nothing is consumed if it cannot be read.
    pub fn get_u64(&mut self) -> Result<u64, Malformed> {
        let (mut result, mut shift) = (0u64, 0u32);
        for (i, &byte) in self.data[self.pos..].iter().enumerate() {
            // the tenth byte holds the sixty-fourth bit and nothing above it
            if shift >= 63 && (byte & 0x7F) > 1 {
                return Err(Malformed("a number of more than 64 bits"));
            }
            result |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                self.pos += i + 1;
                return Ok(result);
            }
            shift += 7;
            if shift > 63 {
                return Err(Malformed("a number of more than 64 bits"));
            }
        }
        Err(Malformed("the bytes end inside a number"))
    }
    fn narrowed(&mut self, most: u64, what: &'static str) -> Result<u64, Malformed> {
        let at = self.pos;
        let v = self.get_u64()?;
        if v > most {
            self.pos = at;
            return Err(Malformed(what));
        }
        Ok(v)
    }
    pub fn get_u16(&mut self) -> Result<u16, Malformed> {
        Ok(self.narrowed(u16::MAX as u64, "a number too large for 16 bits")? as u16)
    }
    /// A count or a length: the reference node reads these as a signed 32-bit number and refuses a negative one,
    /// so nothing above `i32::MAX` is one.
    pub fn get_u32_exact(&mut self) -> Result<u32, Malformed> {
        Ok(self.narrowed(i32::MAX as u64, "a length of more than 31 bits")? as u32)
    }
    /// A signed 32-bit number: ZigZag over the low 32 bits of the VLQ number.
    pub fn get_i32(&mut self) -> Result<i32, Malformed> {
        let n = self.get_u64()? as u32;
        Ok(((n >> 1) as i32) ^ (-((n & 1) as i32)))
    }
}

/// Writes them.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}
impl Writer {
    pub fn new() -> Self {
        Writer::default()
    }
    pub fn result(self) -> Vec<u8> {
        self.buf
    }
    pub fn put_u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn put_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    pub fn put_u64(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return;
            }
            self.buf.push(byte | 0x80);
        }
    }
    pub fn put_u16(&mut self, v: u16) {
        self.put_u64(v as u64);
    }
    pub fn put_u32(&mut self, v: u32) {
        self.put_u64(v as u64);
    }
    /// ZigZag, then widened as the JVM widens an `Int` to a `Long` (the sign carried up), then VLQ: a number
    /// whose ZigZag form has its top bit set is ten bytes on the wire, as the reference node writes it.
    pub fn put_i32(&mut self, v: i32) {
        let zigzag = (v << 1) ^ (v >> 31);
        self.put_u64(zigzag as i64 as u64);
    }
}

// ---- a message's frame ----

/// A message as it crosses: its code and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFrame {
    pub code: u8,
    pub payload: Vec<u8>,
}

/// The network's four bytes, the message's code and its length.
pub const HEADER_LENGTH: usize = 9;
/// The first four bytes of the body's Blake2b-256 hash; sent only when there is a body.
pub const CHECKSUM_LENGTH: usize = 4;
/// The largest body taken from a peer (the vendored codec's limit, which is the reference node's with room to spare).
pub const MAX_PAYLOAD_SIZE: usize = 8_194_304;
pub const MAINNET_MAGIC: [u8; 4] = [1, 0, 2, 4];

fn checksum(payload: &[u8]) -> [u8; CHECKSUM_LENGTH] {
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(payload);
    let hash = hasher.finalize();
    [hash[0], hash[1], hash[2], hash[3]]
}

/// A frame's bytes: magic, code, the body's length as four bytes (big-endian), and, if there is a body, its
/// checksum and the body.
pub fn serialize_frame(magic: &[u8; 4], frame: &MessageFrame) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LENGTH + CHECKSUM_LENGTH + frame.payload.len());
    buf.extend_from_slice(magic);
    buf.push(frame.code);
    buf.extend_from_slice(&(frame.payload.len() as i32).to_be_bytes());
    if !frame.payload.is_empty() {
        buf.extend_from_slice(&checksum(&frame.payload));
        buf.extend_from_slice(&frame.payload);
    }
    buf
}

/// The first whole frame in `data` and how many bytes it took; `None` if `data` does not yet hold one.
pub fn deserialize_frame(
    magic: &[u8; 4],
    data: &[u8],
) -> Result<Option<(MessageFrame, usize)>, Malformed> {
    if data.len() < HEADER_LENGTH {
        return Ok(None);
    }
    let length = i32::from_be_bytes([data[5], data[6], data[7], data[8]]);
    if length < 0 {
        return Err(Malformed("a frame with a negative length"));
    }
    if data[..4] != magic[..] {
        return Err(Malformed("a frame from another network"));
    }
    let (code, length) = (data[4], length as usize);
    if length > MAX_PAYLOAD_SIZE {
        return Err(Malformed("a frame larger than any message"));
    }
    if length == 0 {
        return Ok(Some((
            MessageFrame {
                code,
                payload: Vec::new(),
            },
            HEADER_LENGTH,
        )));
    }
    let total = HEADER_LENGTH + CHECKSUM_LENGTH + length;
    if data.len() < total {
        return Ok(None);
    }
    let payload = &data[HEADER_LENGTH + CHECKSUM_LENGTH..total];
    if checksum(payload)[..] != data[HEADER_LENGTH..HEADER_LENGTH + CHECKSUM_LENGTH] {
        return Err(Malformed("a frame whose checksum is not its body's"));
    }
    Ok(Some((
        MessageFrame {
            code,
            payload: payload.to_vec(),
        },
        total,
    )))
}

/// Why a message could not be read or sent.
#[derive(Debug)]
pub enum ConnectionError {
    Io(std::io::Error),
    Closed,
    Frame(Malformed),
}
impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::Io(e) => write!(f, "io error: {e}"),
            ConnectionError::Closed => f.write_str("connection closed by peer"),
            ConnectionError::Frame(e) => write!(f, "frame error: {e}"),
        }
    }
}
impl std::error::Error for ConnectionError {}

/// A connection to a peer after the handshake: whole messages in and out.
///
/// It keeps no clock of its own. Every read here is made under a time limit by whoever asks (`tokio::time::timeout`
/// round each `read_message`), so a peer that sends half a frame and stops is given up on when that limit ends.
/// What a peer can make it hold is one frame, of at most `MAX_PAYLOAD_SIZE`, and it grows only as the bytes arrive.
pub struct Connection {
    stream: TcpStream,
    magic: [u8; 4],
    read_buf: Vec<u8>,
}
impl Connection {
    /// `initial_data`: what came after the peer's handshake in the same read.
    pub fn new_with_buffer(stream: TcpStream, magic: [u8; 4], initial_data: Vec<u8>) -> Self {
        Connection {
            stream,
            magic,
            read_buf: initial_data,
        }
    }
    /// The next whole message. After an error the connection is of no more use: what was buffered is dropped.
    pub async fn read_message(&mut self) -> Result<MessageFrame, ConnectionError> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match deserialize_frame(&self.magic, &self.read_buf) {
                Ok(Some((frame, used))) => {
                    self.read_buf.drain(..used);
                    return Ok(frame);
                }
                Ok(None) => {}
                Err(e) => {
                    self.read_buf = Vec::new();
                    return Err(ConnectionError::Frame(e));
                }
            }
            let n = self
                .stream
                .read(&mut chunk)
                .await
                .map_err(ConnectionError::Io)?;
            if n == 0 {
                return Err(ConnectionError::Closed);
            }
            self.read_buf.extend_from_slice(&chunk[..n]);
        }
    }
    pub async fn send(&mut self, code: u8, payload: Vec<u8>) -> Result<(), ConnectionError> {
        let bytes = serialize_frame(&self.magic, &MessageFrame { code, payload });
        self.stream
            .write_all(&bytes)
            .await
            .map_err(ConnectionError::Io)
    }
}

// ---- the handshake ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
}
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

pub const FEATURE_LOCAL_ADDRESS: u8 = 2;
pub const FEATURE_SESSION_ID: u8 = 3;
pub const FEATURE_REST_API_URL: u8 = 4;
pub const FEATURE_MODE: u8 = 16;

/// What a peer says of itself beyond its name and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerFeature {
    LocalAddress {
        addr: [u8; 4],
        port: u32,
    },
    SessionId {
        magic: [u8; 4],
        session_id: i64,
    },
    RestApiUrl {
        url: String,
    },
    /// `state_type`: 0 the UTXO set, 1 digests only. `nipopow`: `Some(1)` if it began from a NiPoPoW proof.
    /// `blocks_to_keep`: -1 all of them, -2 from a UTXO snapshot, above zero that many of the newest.
    Mode {
        state_type: u8,
        verify_tx: bool,
        nipopow: Option<i32>,
        blocks_to_keep: i32,
    },
    /// One this client does not know, or one of the above that does not read as it should: kept as it came.
    Unknown {
        feature_id: u8,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredAddress {
    /// four bytes (IPv4) or sixteen (IPv6)
    pub addr: Vec<u8>,
    pub port: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSpec {
    pub agent_name: String,
    pub version: Version,
    pub node_name: String,
    pub declared_address: Option<DeclaredAddress>,
    pub features: Vec<PeerFeature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// milliseconds since 1970, by the sender's clock
    pub time: u64,
    pub peer_spec: PeerSpec,
}

/// A handshake is its own message with no frame round it, and no longer than this.
pub const MAX_HANDSHAKE_SIZE: usize = 8096;

/// At most 255 bytes of a name, cut between characters and not inside one.
fn truncate_to_u8(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len().min(u8::MAX as usize);
    if end < bytes.len() {
        while end > 0 && bytes[end] & 0xC0 == 0x80 {
            end -= 1;
        }
    }
    &bytes[..end]
}

fn feature_body(feature: &PeerFeature) -> (u8, Vec<u8>) {
    let mut w = Writer::new();
    let id = match feature {
        PeerFeature::LocalAddress { addr, port } => {
            w.put_bytes(addr);
            w.put_u32(*port);
            FEATURE_LOCAL_ADDRESS
        }
        PeerFeature::SessionId { magic, session_id } => {
            w.put_bytes(magic);
            w.put_bytes(&session_id.to_be_bytes());
            FEATURE_SESSION_ID
        }
        PeerFeature::RestApiUrl { url } => {
            let url = truncate_to_u8(url.as_bytes());
            w.put_u8(url.len() as u8);
            w.put_bytes(url);
            FEATURE_REST_API_URL
        }
        PeerFeature::Mode {
            state_type,
            verify_tx,
            nipopow,
            blocks_to_keep,
        } => {
            w.put_u8(*state_type);
            w.put_u8(*verify_tx as u8);
            match nipopow {
                Some(n) => {
                    w.put_u8(1);
                    w.put_i32(*n);
                }
                None => w.put_u8(0),
            }
            w.put_i32(*blocks_to_keep);
            FEATURE_MODE
        }
        PeerFeature::Unknown { feature_id, data } => {
            w.put_bytes(data);
            *feature_id
        }
    };
    (id, w.result())
}

/// `scheme://…`, the scheme of letters, digits and `+-.`: what a REST address must at least look like to be kept as one.
fn has_url_scheme(s: &str) -> bool {
    match s.find("://") {
        Some(at) if at > 0 => s[..at]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.'),
        _ => false,
    }
}

/// A feature as its id says to read it. One that does not read so is kept as `Unknown`, never refused: a peer's
/// handshake is not thrown away for one field of it. (The mode's fields, as the vendored codec has them, fall back
/// one by one: a mode cut short reads as a node that keeps every block.)
fn feature_read(feature_id: u8, data: &[u8]) -> PeerFeature {
    let unknown = || PeerFeature::Unknown {
        feature_id,
        data: data.to_vec(),
    };
    match feature_id {
        FEATURE_LOCAL_ADDRESS => {
            if data.len() < 5 {
                return unknown();
            }
            PeerFeature::LocalAddress {
                addr: [data[0], data[1], data[2], data[3]],
                port: Reader::new(&data[4..]).get_u32_exact().unwrap_or(0),
            }
        }
        FEATURE_SESSION_ID => {
            if data.len() < 12 {
                return unknown();
            }
            let mut id = [0u8; 8];
            id.copy_from_slice(&data[4..12]);
            PeerFeature::SessionId {
                magic: [data[0], data[1], data[2], data[3]],
                session_id: i64::from_be_bytes(id),
            }
        }
        FEATURE_REST_API_URL => {
            let Some((&len, rest)) = data.split_first() else {
                return unknown();
            };
            match rest.get(..len as usize).map(std::str::from_utf8) {
                Some(Ok(url)) if !url.is_empty() && has_url_scheme(url) => {
                    PeerFeature::RestApiUrl {
                        url: url.to_string(),
                    }
                }
                _ => unknown(),
            }
        }
        FEATURE_MODE => {
            let mut r = Reader::new(data);
            let state_type = r.get_u8().unwrap_or(0);
            let verify_tx = r.get_u8().is_ok_and(|b| b > 0);
            let nipopow = match r.get_u8() {
                Ok(1) => r.get_i32().ok(),
                _ => None,
            };
            PeerFeature::Mode {
                state_type,
                verify_tx,
                nipopow,
                blocks_to_keep: r.get_i32().unwrap_or(-1),
            }
        }
        _ => unknown(),
    }
}

fn peer_spec_to(spec: &PeerSpec, w: &mut Writer) {
    let agent = truncate_to_u8(spec.agent_name.as_bytes());
    w.put_u8(agent.len() as u8);
    w.put_bytes(agent);
    w.put_u8(spec.version.major);
    w.put_u8(spec.version.minor);
    w.put_u8(spec.version.patch);
    let name = truncate_to_u8(spec.node_name.as_bytes());
    w.put_u8(name.len() as u8);
    w.put_bytes(name);
    match &spec.declared_address {
        Some(a) => {
            w.put_u8(1);
            // the length the reference node writes: the address's bytes and four for the port, whatever the port takes
            w.put_u8((a.addr.len() + 4) as u8);
            w.put_bytes(&a.addr);
            w.put_u32(a.port);
        }
        None => w.put_u8(0),
    }
    // the count is a signed byte to the reference node: no more than 127 are written
    let most = (u8::MAX as usize) / 2;
    w.put_u8(spec.features.len().min(most) as u8);
    for feature in spec.features.iter().take(most) {
        let (id, body) = feature_body(feature);
        w.put_u8(id);
        w.put_u16(body.len() as u16);
        w.put_bytes(&body);
    }
}

fn peer_spec_from(r: &mut Reader) -> Result<PeerSpec, Malformed> {
    let agent_len = r.get_u8()? as usize;
    let agent_name = String::from_utf8_lossy(r.get_bytes(agent_len)?).to_string();
    if agent_name.is_empty() {
        return Err(Malformed("a handshake with no agent name"));
    }
    let version = Version {
        major: r.get_u8()?,
        minor: r.get_u8()?,
        patch: r.get_u8()?,
    };
    let name_len = r.get_u8()? as usize;
    let node_name = String::from_utf8_lossy(r.get_bytes(name_len)?).to_string();
    let declared_address = if r.get_u8()? == 1 {
        let addr_len = (r.get_u8()? as usize).saturating_sub(4);
        Some(DeclaredAddress {
            addr: r.get_bytes(addr_len)?.to_vec(),
            port: r.get_u32_exact()?,
        })
    } else {
        None
    };
    let count = r.get_u8()? as i8;
    if count < 0 {
        return Err(Malformed("a handshake with a negative count of features"));
    }
    let mut features = Vec::new();
    for _ in 0..count {
        let id = r.get_u8()?;
        let len = r.get_u16()? as usize;
        features.push(feature_read(id, r.get_bytes(len)?));
    }
    Ok(PeerSpec {
        agent_name,
        version,
        node_name,
        declared_address,
        features,
    })
}

pub fn serialize_handshake(hs: &Handshake) -> Vec<u8> {
    let mut w = Writer::new();
    w.put_u64(hs.time);
    peer_spec_to(&hs.peer_spec, &mut w);
    w.result()
}

/// A handshake from the start of `payload`, and how many bytes it took (a peer may send its first message in the
/// same packet). An error may only mean the rest has not arrived yet: the caller reads more and asks again.
pub fn deserialize_handshake_with_consumed(
    payload: &[u8],
) -> Result<(Handshake, usize), Malformed> {
    if payload.len() > MAX_HANDSHAKE_SIZE {
        return Err(Malformed("a handshake larger than any handshake"));
    }
    let mut r = Reader::new(payload);
    let time = r.get_u64()?;
    let peer_spec = peer_spec_from(&mut r)?;
    Ok((Handshake { time, peer_spec }, r.position()))
}

pub fn deserialize_handshake(payload: &[u8]) -> Result<Handshake, Malformed> {
    deserialize_handshake_with_consumed(payload).map(|(hs, _)| hs)
}

// ---- messages ----

pub const CODE_REQUEST_MODIFIER: u8 = 22;
pub const CODE_MODIFIER: u8 = 33;
pub const CODE_INV: u8 = 55;
pub const CODE_SYNC_INFO: u8 = 65;
pub const CODE_GET_NIPOPOW_PROOF: u8 = 90;
pub const CODE_NIPOPOW_PROOF: u8 = 91;

const MAX_INV_OBJECTS: usize = 400;
/// Four times the reference node's largest modifiers message: what is taken from a peer, and the most that is sent.
const MAX_MODIFIERS_BYTES: usize = 2_048_576 * 4;
const MAX_SYNC_V1_IDS: usize = 1000;
const MAX_SYNC_V2_HEADERS: usize = 50;
const MAX_HEADER_SIZE: usize = 1000;

/// Ids of one kind of thing (headers are 101): what a peer has (`Inv`) or what is asked of it (`RequestModifier`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvData {
    pub type_id: u8,
    pub ids: Vec<[u8; 32]>,
}

pub fn serialize_inv(data: &InvData) -> Result<Vec<u8>, Malformed> {
    if data.ids.is_empty() {
        return Err(Malformed("an inventory of nothing"));
    }
    if data.ids.len() > MAX_INV_OBJECTS {
        return Err(Malformed("an inventory of more than 400 ids"));
    }
    let mut w = Writer::new();
    w.put_u8(data.type_id);
    w.put_u32(data.ids.len() as u32);
    for id in &data.ids {
        w.put_bytes(id);
    }
    Ok(w.result())
}

pub fn deserialize_inv(payload: &[u8]) -> Result<InvData, Malformed> {
    let mut r = Reader::new(payload);
    let type_id = r.get_u8()?;
    let count = r.get_u32_exact()? as usize;
    if count == 0 {
        return Err(Malformed("an inventory of nothing"));
    }
    if count > MAX_INV_OBJECTS {
        return Err(Malformed("an inventory of more than 400 ids"));
    }
    let mut ids = Vec::with_capacity(count.min(r.remaining() / 32));
    for _ in 0..count {
        ids.push(r.get_id()?);
    }
    Ok(InvData { type_id, ids })
}

/// Things of one kind, each with its id: headers, as a peer sends them when asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifiersData {
    pub type_id: u8,
    pub modifiers: Vec<([u8; 32], Vec<u8>)>,
}

/// As many of them, from the first, as fit a message; the rest are left out, as the reference node leaves them.
pub fn serialize_modifiers(data: &ModifiersData) -> Result<Vec<u8>, Malformed> {
    if data.modifiers.is_empty() {
        return Err(Malformed("a modifiers message of nothing"));
    }
    // each counted as its id, four bytes for its length and its bytes, after five for the kind and the count
    let (mut size, mut fit) = (5usize, 0usize);
    for (_, modifier) in &data.modifiers {
        let entry = 32 + 4 + modifier.len();
        if size + entry <= MAX_MODIFIERS_BYTES {
            fit += 1;
        }
        size += entry;
    }
    if fit == 0 {
        return Err(Malformed("a modifier too large for any message"));
    }
    let mut w = Writer::new();
    w.put_u8(data.type_id);
    w.put_u32(fit as u32);
    for (id, modifier) in data.modifiers.iter().take(fit) {
        w.put_bytes(id);
        w.put_u32(modifier.len() as u32);
        w.put_bytes(modifier);
    }
    Ok(w.result())
}

pub fn deserialize_modifiers(payload: &[u8]) -> Result<ModifiersData, Malformed> {
    let mut r = Reader::new(payload);
    let type_id = r.get_u8()?;
    let count = r.get_u32_exact()? as usize;
    if count == 0 {
        return Err(Malformed("a modifiers message of nothing"));
    }
    let mut size = 5usize;
    // room is made for what the bytes in hand could hold (an id and a length each at the least), not for the count given
    let mut modifiers = Vec::with_capacity(count.min(r.remaining() / 33));
    for _ in 0..count {
        let id = r.get_id()?;
        let len = r.get_u32_exact()? as usize;
        size += 32 + 4 + len;
        if size > MAX_MODIFIERS_BYTES {
            return Err(Malformed("a modifiers message larger than any"));
        }
        modifiers.push((id, r.get_bytes(len)?.to_vec()));
    }
    Ok(ModifiersData { type_id, modifiers })
}

/// Where a peer's chain stands: the ids of its newest headers (the older form) or the headers themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncInfo {
    V1 { header_ids: Vec<[u8; 32]> },
    V2 { headers: Vec<Vec<u8>> },
}

pub fn serialize_sync_info(info: &SyncInfo) -> Result<Vec<u8>, Malformed> {
    let mut w = Writer::new();
    match info {
        SyncInfo::V1 { header_ids } => {
            if header_ids.len() > MAX_SYNC_V1_IDS + 1 {
                return Err(Malformed("sync info with more ids than any"));
            }
            w.put_u16(header_ids.len() as u16);
            for id in header_ids {
                w.put_bytes(id);
            }
        }
        SyncInfo::V2 { headers } => {
            if headers.len() > MAX_SYNC_V2_HEADERS {
                return Err(Malformed("sync info with more headers than any"));
            }
            // no ids, then a byte of -1: how the newer form is told from an older one with nothing in it
            w.put_u16(0);
            w.put_u8(0xFF);
            w.put_u8(headers.len() as u8);
            for header in headers {
                if header.len() > MAX_HEADER_SIZE {
                    return Err(Malformed("sync info with a header larger than any"));
                }
                w.put_u16(header.len() as u16);
                w.put_bytes(header);
            }
        }
    }
    Ok(w.result())
}

pub fn deserialize_sync_info(payload: &[u8]) -> Result<SyncInfo, Malformed> {
    let mut r = Reader::new(payload);
    let length = r.get_u16()? as usize;
    if length > 0 {
        if length > MAX_SYNC_V1_IDS + 1 {
            return Err(Malformed("sync info with more ids than any"));
        }
        let mut header_ids = Vec::with_capacity(length.min(r.remaining() / 32));
        for _ in 0..length {
            header_ids.push(r.get_id()?);
        }
        return Ok(SyncInfo::V1 { header_ids });
    }
    if r.remaining() == 0 {
        return Ok(SyncInfo::V1 {
            header_ids: Vec::new(),
        });
    }
    if r.get_u8()? != 0xFF {
        return Err(Malformed("sync info of a form that is not known"));
    }
    let count = r.get_u8()? as usize;
    if count > MAX_SYNC_V2_HEADERS {
        return Err(Malformed("sync info with more headers than any"));
    }
    let mut headers = Vec::with_capacity(count);
    for _ in 0..count {
        let len = r.get_u16()? as usize;
        if len > MAX_HEADER_SIZE {
            return Err(Malformed("sync info with a header larger than any"));
        }
        headers.push(r.get_bytes(len)?.to_vec());
    }
    Ok(SyncInfo::V2 { headers })
}

/// What a NiPoPoW proof is asked with: its two parameters, and the header it should end at (the tip if none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NipopowProofData {
    pub m: i32,
    pub k: i32,
    pub header_id_opt: Option<[u8; 32]>,
}

pub fn serialize_get_nipopow_proof(data: &NipopowProofData) -> Vec<u8> {
    let mut w = Writer::new();
    w.put_i32(data.m);
    w.put_i32(data.k);
    match &data.header_id_opt {
        Some(id) => {
            w.put_u8(1);
            w.put_bytes(id);
        }
        None => w.put_u8(0),
    }
    // the length of what a later version may add: nothing
    w.put_u16(0);
    w.result()
}

pub fn deserialize_get_nipopow_proof(payload: &[u8]) -> Result<NipopowProofData, Malformed> {
    if payload.len() > 1000 {
        return Err(Malformed("a proof request larger than any"));
    }
    let mut r = Reader::new(payload);
    let (m, k) = (r.get_i32()?, r.get_i32()?);
    let header_id_opt = if r.get_u8()? == 1 {
        Some(r.get_id()?)
    } else {
        None
    };
    let pad = r.get_u16()? as usize;
    if pad > 0 && pad < 1000 {
        r.get_bytes(pad)?;
    }
    Ok(NipopowProofData {
        m,
        k,
        header_id_opt,
    })
}

pub fn serialize_nipopow_proof(proof: &[u8]) -> Result<Vec<u8>, Malformed> {
    if proof.is_empty() {
        return Err(Malformed("a proof of no bytes"));
    }
    let mut w = Writer::new();
    w.put_u32(proof.len() as u32);
    w.put_bytes(proof);
    w.put_u16(0);
    let out = w.result();
    if out.len() > 2_000_000 {
        return Err(Malformed("a proof larger than any"));
    }
    Ok(out)
}

/// The proof's bytes. What a later version added after them is read past, if it is all there.
pub fn deserialize_nipopow_proof(payload: &[u8]) -> Result<Vec<u8>, Malformed> {
    if payload.len() > 2_000_000 {
        return Err(Malformed("a proof larger than any"));
    }
    let mut r = Reader::new(payload);
    let len = r.get_u32_exact()? as usize;
    if len == 0 {
        return Err(Malformed("a proof of no bytes"));
    }
    let proof = r.get_bytes(len)?;
    let pad = r.get_u16()? as usize;
    if pad > 0 && pad < 2_000_000 {
        r.get_bytes(pad)?;
    }
    Ok(proof.to_vec())
}
