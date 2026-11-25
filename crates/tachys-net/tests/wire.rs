//! The wallet's own codec (`src/wire.rs`) held to the vendored one it took the place of: the same bytes out for the
//! same thing, the same thing read from the same bytes, and the same bytes refused, on values and bytes made at
//! random (the same ones every run) and on valid messages damaged a byte at a time. No network beyond this machine.
use ergo_p2p::{framing as vf, handshake as vh, message as vm, types as vt};
use ergo_primitives::reader::VlqReader;
use ergo_primitives::writer::VlqWriter;
use tachys_net::wire::*;

/// Numbers that are the same every run (xorshift64*): a failure can be run again.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn id(&mut self) -> [u8; 32] {
        let mut id = [0u8; 32];
        id.copy_from_slice(&self.bytes(32));
        id
    }
    /// A number of any size: as many of its low bits as a throw of the dice says, so that small ones are as likely as large.
    fn number(&mut self) -> u64 {
        let bits = self.below(65);
        if bits == 0 {
            0
        } else {
            self.next() >> (64 - bits)
        }
    }
    /// Valid bytes with one thing wrong: a byte changed, bytes cut off the end, a byte put in, or bytes added.
    fn damaged(&mut self, good: &[u8]) -> Vec<u8> {
        let mut b = good.to_vec();
        match self.below(5) {
            0 if !b.is_empty() => {
                let at = self.below(b.len());
                b[at] = self.next() as u8;
            }
            1 if !b.is_empty() => b.truncate(self.below(b.len())),
            2 => {
                let at = self.below(b.len() + 1);
                b.insert(at, self.next() as u8);
            }
            3 => {
                let more = self.below(40);
                b.extend(self.bytes(more));
            }
            _ => {
                let at = self.below(b.len() + 1);
                b.splice(at..at, [0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
            }
        }
        b
    }
}

// ---- the vendored codec's values, as this one's ----

fn feature(f: &vh::PeerFeature) -> PeerFeature {
    match f.clone() {
        vh::PeerFeature::LocalAddress { addr, port } => PeerFeature::LocalAddress { addr, port },
        vh::PeerFeature::SessionId { magic, session_id } => {
            PeerFeature::SessionId { magic, session_id }
        }
        vh::PeerFeature::RestApiUrl { url } => PeerFeature::RestApiUrl { url },
        vh::PeerFeature::Mode {
            state_type,
            verify_tx,
            nipopow,
            blocks_to_keep,
        } => PeerFeature::Mode {
            state_type,
            verify_tx,
            nipopow,
            blocks_to_keep,
        },
        vh::PeerFeature::Unknown { feature_id, data } => PeerFeature::Unknown { feature_id, data },
    }
}
fn their_feature(f: &PeerFeature) -> vh::PeerFeature {
    match f.clone() {
        PeerFeature::LocalAddress { addr, port } => vh::PeerFeature::LocalAddress { addr, port },
        PeerFeature::SessionId { magic, session_id } => {
            vh::PeerFeature::SessionId { magic, session_id }
        }
        PeerFeature::RestApiUrl { url } => vh::PeerFeature::RestApiUrl { url },
        PeerFeature::Mode {
            state_type,
            verify_tx,
            nipopow,
            blocks_to_keep,
        } => vh::PeerFeature::Mode {
            state_type,
            verify_tx,
            nipopow,
            blocks_to_keep,
        },
        PeerFeature::Unknown { feature_id, data } => vh::PeerFeature::Unknown { feature_id, data },
    }
}
fn handshake(h: &vh::Handshake) -> Handshake {
    let s = &h.peer_spec;
    Handshake {
        time: h.time,
        peer_spec: PeerSpec {
            agent_name: s.agent_name.clone(),
            version: Version {
                major: s.version.major,
                minor: s.version.minor,
                patch: s.version.patch,
            },
            node_name: s.node_name.clone(),
            declared_address: s.declared_address.as_ref().map(|a| DeclaredAddress {
                addr: a.addr.clone(),
                port: a.port,
            }),
            features: s.features.iter().map(feature).collect(),
        },
    }
}
fn their_handshake(h: &Handshake) -> vh::Handshake {
    let s = &h.peer_spec;
    vh::Handshake {
        time: h.time,
        peer_spec: vh::PeerSpec {
            agent_name: s.agent_name.clone(),
            version: vh::Version {
                major: s.version.major,
                minor: s.version.minor,
                patch: s.version.patch,
            },
            node_name: s.node_name.clone(),
            declared_address: s.declared_address.as_ref().map(|a| vh::DeclaredAddress {
                addr: a.addr.clone(),
                port: a.port,
            }),
            features: s.features.iter().map(their_feature).collect(),
        },
    }
}
fn sync(s: &vm::SyncInfo) -> SyncInfo {
    match s.clone() {
        vm::SyncInfo::V1 { header_ids } => SyncInfo::V1 { header_ids },
        vm::SyncInfo::V2 { headers } => SyncInfo::V2 { headers },
    }
}
fn their_sync(s: &SyncInfo) -> vm::SyncInfo {
    match s.clone() {
        SyncInfo::V1 { header_ids } => vm::SyncInfo::V1 { header_ids },
        SyncInfo::V2 { headers } => vm::SyncInfo::V2 { headers },
    }
}

// ---- things made at random ----

fn text(r: &mut Rng, most: usize) -> String {
    let n = r.below(most + 1);
    (0..n)
        .map(|_| ['a', 'Z', '7', '-', ' ', 'é', '日', '🔑'][r.below(8)])
        .collect()
}
fn some_feature(r: &mut Rng) -> PeerFeature {
    match r.below(6) {
        0 => PeerFeature::LocalAddress {
            addr: [
                r.next() as u8,
                r.next() as u8,
                r.next() as u8,
                r.next() as u8,
            ],
            port: r.number() as u32 & 0x7FFF_FFFF,
        },
        1 => PeerFeature::SessionId {
            magic: [
                r.next() as u8,
                r.next() as u8,
                r.next() as u8,
                r.next() as u8,
            ],
            session_id: r.next() as i64,
        },
        2 => PeerFeature::RestApiUrl {
            url: format!("http{}://{}", ["", "s"][r.below(2)], text(r, 300)),
        },
        3 | 4 => PeerFeature::Mode {
            state_type: r.below(3) as u8,
            verify_tx: r.below(2) == 1,
            nipopow: [None, Some(1), Some(r.next() as i32)][r.below(3)],
            blocks_to_keep: [-1, -2, 0, 1440, r.next() as i32, i32::MIN, i32::MAX][r.below(7)],
        },
        // an id this client does not know, or one it does with bytes that are not that feature's
        _ => {
            let n = r.below(20);
            PeerFeature::Unknown {
                feature_id: [0, 1, 5, 15, 17, 200, 255][r.below(7)],
                data: r.bytes(n),
            }
        }
    }
}
fn some_handshake(r: &mut Rng) -> Handshake {
    let n = r.below(5);
    Handshake {
        time: r.number(),
        peer_spec: PeerSpec {
            agent_name: format!("a{}", text(r, 300)),
            version: Version {
                major: r.next() as u8,
                minor: r.next() as u8,
                patch: r.next() as u8,
            },
            node_name: text(r, 300),
            declared_address: match r.below(3) {
                0 => None,
                1 => Some(DeclaredAddress {
                    addr: r.bytes(4),
                    port: r.number() as u32 & 0x7FFF_FFFF,
                }),
                _ => Some(DeclaredAddress {
                    addr: r.bytes(16),
                    port: r.below(65536) as u32,
                }),
            },
            features: (0..n).map(|_| some_feature(r)).collect(),
        },
    }
}

// ---- whole numbers ----

#[test]
fn a_number_is_written_as_the_vendored_codec_writes_it() {
    let mut r = Rng(0x1234_5678_9ABC_DEF1);
    let mut edges: Vec<u64> = vec![0, 1, 127, 128, 16383, 16384, u32::MAX as u64, u64::MAX];
    edges.extend((0..64).flat_map(|b| [1u64 << b, (1u64 << b).wrapping_sub(1), (1u64 << b) + 1]));
    for v in edges.into_iter().chain((0..20_000).map(|_| r.number())) {
        let (mut ours, mut theirs) = (Writer::new(), VlqWriter::new());
        ours.put_u64(v);
        ours.put_u32(v as u32);
        ours.put_u16(v as u16);
        ours.put_i32(v as i32);
        ours.put_u8(v as u8);
        theirs.put_u64(v);
        theirs.put_u32(v as u32);
        theirs.put_u16(v as u16);
        theirs.put_i32(v as i32);
        theirs.put_u8(v as u8);
        assert_eq!(ours.result(), theirs.result(), "{v}");
    }
    // a negative number whose ZigZag form has its top bit set is ten bytes, as the reference node writes it
    let mut w = Writer::new();
    w.put_i32(i32::MIN);
    assert_eq!(w.result().len(), 10);
}

#[test]
fn a_number_is_read_as_the_vendored_codec_reads_it_and_refused_where_it_refuses() {
    let mut r = Rng(0xDEAD_BEEF_0BAD_F00D);
    for round in 0..60_000 {
        // bytes at random; or a number as written, perhaps damaged; or a run of bytes that all say "more follows"
        let bytes = match r.below(4) {
            0 => {
                let n = r.below(14);
                r.bytes(n)
            }
            1 => vec![0x80 | r.next() as u8; r.below(13)],
            _ => {
                let mut w = VlqWriter::new();
                w.put_u64(r.number());
                let good = w.result();
                if r.below(2) == 0 {
                    good
                } else {
                    r.damaged(&good)
                }
            }
        };
        macro_rules! same {
            ($read:ident) => {{
                let (mut ours, mut theirs) = (Reader::new(&bytes), VlqReader::new(&bytes));
                let (a, b) = (ours.$read(), theirs.$read());
                assert_eq!(
                    a.ok(),
                    b.ok(),
                    "round {round}, {}: {bytes:02x?}",
                    stringify!($read)
                );
                assert_eq!(
                    ours.position(),
                    theirs.position(),
                    "round {round}, {}: how much was read of {bytes:02x?}",
                    stringify!($read)
                );
                assert_eq!(ours.remaining(), theirs.remaining());
            }};
        }
        same!(get_u64);
        same!(get_u16);
        same!(get_u32_exact);
        same!(get_i32);
        same!(get_u8);
        let n = r.below(20);
        let (mut ours, mut theirs) = (Reader::new(&bytes), VlqReader::new(&bytes));
        assert_eq!(
            ours.get_bytes(n).ok(),
            theirs.get_bytes(n).ok(),
            "round {round}"
        );
        assert_eq!(ours.position(), theirs.position());
    }
    // a length no slice could have is refused, not added to a position and wrapped round
    assert!(Reader::new(&[1, 2, 3]).get_bytes(usize::MAX).is_err());
}

// ---- frames ----

#[test]
fn a_frame_is_the_same_bytes_and_reads_back_the_same_from_either() {
    let mut r = Rng(0x0F0F_F0F0_1357_9BDF);
    for round in 0..4_000 {
        let magic = [
            r.next() as u8,
            r.next() as u8,
            r.next() as u8,
            r.next() as u8,
        ];
        let n = [0, 1, 32, 300, r.below(5000)][r.below(5)];
        let frame = MessageFrame {
            code: r.next() as u8,
            payload: r.bytes(n),
        };
        let bytes = serialize_frame(&magic, &frame);
        let theirs = vf::serialize_frame(
            &magic,
            &vf::MessageFrame {
                code: frame.code,
                payload: frame.payload.clone(),
            },
        );
        assert_eq!(bytes, theirs, "round {round}");
        assert_eq!(
            deserialize_frame(&magic, &bytes),
            Ok(Some((frame.clone(), bytes.len())))
        );
        // with more after it, only the frame is taken; with any of it missing, there is no frame yet
        let mut two = bytes.clone();
        two.extend(serialize_frame(&magic, &frame));
        assert_eq!(
            deserialize_frame(&magic, &two),
            Ok(Some((frame.clone(), bytes.len())))
        );
        let cut = r.below(bytes.len());
        assert_eq!(
            deserialize_frame(&magic, &bytes[..cut]),
            Ok(None),
            "round {round}: cut at {cut}"
        );
        // damaged, or read on another network: the two agree on what it is
        let other = [magic[0] ^ 1, magic[1], magic[2], magic[3]];
        for (m, data) in [
            (magic, r.damaged(&bytes)),
            (other, bytes.clone()),
            (magic, r.bytes(n.min(40))),
        ] {
            let ours = deserialize_frame(&m, &data);
            let theirs = vf::deserialize_frame(&m, &data);
            // a length above the largest message is refused here at once; the vendored codec's connection refuses it
            // (`PayloadTooLarge`), and its frame reader alone would wait for the bytes
            if data.len() >= 9
                && i32::from_be_bytes([data[5], data[6], data[7], data[8]]) as i64
                    > MAX_PAYLOAD_SIZE as i64
                && data[..4] == m
            {
                assert!(ours.is_err(), "round {round}");
                continue;
            }
            match (ours, theirs) {
                (Ok(None), Ok(None)) | (Err(_), Err(_)) => {}
                (Ok(Some((f, used))), Ok(Some((g, theirs_used)))) => {
                    assert_eq!(
                        (f.code, f.payload, used),
                        (g.code, g.payload, theirs_used),
                        "round {round}"
                    )
                }
                (a, b) => panic!(
                    "round {round}: {data:02x?} read here as {a:?} and there as {:?}",
                    b.map(|x| x.map(|y| y.1))
                ),
            }
        }
    }
    assert!(
        deserialize_frame(&MAINNET_MAGIC, &[1, 0, 2, 4, 55, 0x7F, 0xFF, 0xFF, 0xFF]).is_err(),
        "a length no message has"
    );
    assert!(
        deserialize_frame(&MAINNET_MAGIC, &[1, 0, 2, 4, 55, 0xFF, 0xFF, 0xFF, 0xFF]).is_err(),
        "a negative length"
    );
    assert_eq!(MAINNET_MAGIC, vf::MAINNET_MAGIC);
}

// ---- the handshake ----

#[test]
fn a_handshake_is_the_same_bytes_and_reads_back_the_same_from_either() {
    let mut r = Rng(0xA5A5_5A5A_C3C3_3C3C);
    for round in 0..20_000 {
        let hs = some_handshake(&mut r);
        let bytes = serialize_handshake(&hs);
        assert_eq!(
            bytes,
            vh::serialize_handshake(&their_handshake(&hs)),
            "round {round}: {hs:?}"
        );
        // what is read back, by either, from the bytes as written and from those bytes damaged
        for data in [bytes.clone(), r.damaged(&bytes), r.damaged(&bytes), {
            let n = r.below(60);
            r.bytes(n)
        }] {
            let ours = deserialize_handshake_with_consumed(&data);
            let theirs = vh::deserialize_handshake_with_consumed(&data);
            match (ours, theirs) {
                (Err(_), Err(_)) => {}
                (Ok((a, used)), Ok((b, theirs_used))) => {
                    assert_eq!(a, handshake(&b), "round {round}: {data:02x?}");
                    assert_eq!(
                        used, theirs_used,
                        "round {round}: how much of {data:02x?} is the handshake"
                    );
                    assert_eq!(deserialize_handshake(&data), Ok(a));
                }
                (a, b) => {
                    panic!("round {round}: {data:02x?} read here as {a:?} and there as {b:?}")
                }
            }
        }
    }
    // larger than any handshake: refused whole, whatever it holds
    assert!(deserialize_handshake_with_consumed(&vec![1u8; MAX_HANDSHAKE_SIZE + 1]).is_err());
    assert_eq!(MAX_HANDSHAKE_SIZE, vh::MAX_HANDSHAKE_SIZE);
}

#[test]
fn the_handshake_this_wallet_sends_is_read_by_the_vendored_codec_as_what_it_says() {
    // what `our_handshake` sends for a light client: its mode, and the session it is
    let hs = Handshake {
        time: 1_763_000_000_000,
        peer_spec: PeerSpec {
            agent_name: tachys_net::AGENT.into(),
            version: tachys_net::PROTOCOL,
            node_name: tachys_net::AGENT.into(),
            declared_address: None,
            features: vec![
                PeerFeature::Mode {
                    state_type: 1,
                    verify_tx: false,
                    nipopow: Some(1),
                    blocks_to_keep: 0,
                },
                PeerFeature::SessionId {
                    magic: MAINNET_MAGIC,
                    session_id: -42,
                },
            ],
        },
    };
    let read = vh::deserialize_handshake(&serialize_handshake(&hs)).unwrap();
    assert_eq!(read.peer_spec.agent_name, "tachys");
    assert_eq!(
        read.peer_spec.version.to_string(),
        tachys_net::PROTOCOL.to_string()
    );
    assert_eq!(read.peer_spec.features.len(), 2);
    assert!(matches!(
        read.peer_spec.features[0],
        vh::PeerFeature::Mode {
            state_type: 1,
            verify_tx: false,
            nipopow: Some(1),
            blocks_to_keep: 0
        }
    ));
    assert!(matches!(
        read.peer_spec.features[1],
        vh::PeerFeature::SessionId {
            magic: [1, 0, 2, 4],
            session_id: -42
        }
    ));
}

// ---- messages ----

/// One message, written and read by both: `ours` and `theirs` each give what they read of `data`, as this codec's value.
fn read_alike<T: PartialEq + std::fmt::Debug>(
    what: &str,
    round: usize,
    data: &[u8],
    ours: Option<T>,
    theirs: Option<T>,
) {
    assert_eq!(ours, theirs, "round {round}: {what} {data:02x?}");
}

#[test]
fn each_message_is_the_same_bytes_and_reads_back_the_same_from_either() {
    let mut r = Rng(0x6A09_E667_F3BC_C908);
    assert_eq!(
        (
            CODE_REQUEST_MODIFIER,
            CODE_MODIFIER,
            CODE_INV,
            CODE_SYNC_INFO,
            CODE_GET_NIPOPOW_PROOF,
            CODE_NIPOPOW_PROOF
        ),
        (
            vm::CODE_REQUEST_MODIFIER,
            vm::CODE_MODIFIER,
            vm::CODE_INV,
            vm::CODE_SYNC_INFO,
            vm::CODE_GET_NIPOPOW_PROOF,
            vm::CODE_NIPOPOW_PROOF
        )
    );
    for round in 0..6_000 {
        let random = {
            let n = r.below(80);
            r.bytes(n)
        };

        // inventory: none, some, the most there may be, and one more than that
        let n = [0, 1, 3, 400, 401, r.below(30)][r.below(6)];
        let inv = InvData {
            type_id: r.next() as u8,
            ids: (0..n).map(|_| r.id()).collect(),
        };
        let ours = serialize_inv(&inv);
        let theirs = vm::serialize_inv(&vt::InvData {
            type_id: inv.type_id,
            ids: inv.ids.clone(),
        });
        assert_eq!(
            ours.clone().ok(),
            theirs.ok(),
            "round {round}: an inventory of {n}"
        );
        for data in [
            ours.clone().unwrap_or_default(),
            r.damaged(&ours.unwrap_or_default()),
            random.clone(),
        ] {
            read_alike(
                "inv",
                round,
                &data,
                deserialize_inv(&data).ok(),
                vm::deserialize_inv(&data).ok().map(|d| InvData {
                    type_id: d.type_id,
                    ids: d.ids,
                }),
            );
        }

        // modifiers
        let n = [0, 1, 2, r.below(12)][r.below(4)];
        let mods = ModifiersData {
            type_id: r.next() as u8,
            modifiers: (0..n)
                .map(|_| {
                    let len = [0, 1, 220, r.below(600)][r.below(4)];
                    (r.id(), r.bytes(len))
                })
                .collect(),
        };
        let ours = serialize_modifiers(&mods);
        let theirs = vm::serialize_modifiers(&vt::ModifiersData {
            type_id: mods.type_id,
            modifiers: mods.modifiers.clone(),
        });
        assert_eq!(
            ours.clone().ok(),
            theirs.ok(),
            "round {round}: {n} modifiers"
        );
        for data in [
            ours.clone().unwrap_or_default(),
            r.damaged(&ours.unwrap_or_default()),
            random.clone(),
        ] {
            read_alike(
                "modifiers",
                round,
                &data,
                deserialize_modifiers(&data).ok(),
                vm::deserialize_modifiers(&data)
                    .ok()
                    .map(|d| ModifiersData {
                        type_id: d.type_id,
                        modifiers: d.modifiers,
                    }),
            );
        }

        // sync info, both forms, at and over each form's limits
        let info = if r.below(2) == 0 {
            let n = [0, 1, 10, 1001, 1002][r.below(5)];
            SyncInfo::V1 {
                header_ids: (0..n).map(|_| r.id()).collect(),
            }
        } else {
            let n = [0, 1, 4, 50, 51][r.below(5)];
            // (one header too long in a few of them only: with one in most, a message of fifty was never written)
            let long = if r.below(6) == 0 {
                r.below(n.max(1))
            } else {
                usize::MAX
            };
            SyncInfo::V2 {
                headers: (0..n)
                    .map(|i| {
                        let len = if i == long {
                            1001
                        } else {
                            [0, 1, 220, 1000][r.below(4)]
                        };
                        r.bytes(len)
                    })
                    .collect(),
            }
        };
        let ours = serialize_sync_info(&info);
        assert_eq!(
            ours.clone().ok(),
            vm::serialize_sync_info(&their_sync(&info)).ok(),
            "round {round}: {info:?}"
        );
        for data in [
            ours.clone().unwrap_or_default(),
            r.damaged(&ours.unwrap_or_default()),
            random.clone(),
        ] {
            read_alike(
                "sync info",
                round,
                &data,
                deserialize_sync_info(&data).ok(),
                vm::deserialize_sync_info(&data).ok().map(|s| sync(&s)),
            );
        }

        // the request for a proof, and the proof
        let ask = NipopowProofData {
            m: [6, 0, -1, r.next() as i32][r.below(4)],
            k: [10, r.next() as i32][r.below(2)],
            header_id_opt: if r.below(2) == 0 { None } else { Some(r.id()) },
        };
        let ours = serialize_get_nipopow_proof(&ask);
        assert_eq!(
            ours,
            vm::serialize_get_nipopow_proof(&vt::NipopowProofData {
                m: ask.m,
                k: ask.k,
                header_id_opt: ask.header_id_opt
            }),
            "round {round}"
        );
        for data in [ours.clone(), r.damaged(&ours), random.clone()] {
            read_alike(
                "proof request",
                round,
                &data,
                deserialize_get_nipopow_proof(&data).ok(),
                vm::deserialize_get_nipopow_proof(&data)
                    .ok()
                    .map(|d| NipopowProofData {
                        m: d.m,
                        k: d.k,
                        header_id_opt: d.header_id_opt,
                    }),
            );
        }
        let proof = {
            let n = [0, 1, 700, r.below(3000)][r.below(4)];
            r.bytes(n)
        };
        let ours = serialize_nipopow_proof(&proof);
        assert_eq!(
            ours.clone().ok(),
            vm::serialize_nipopow_proof(&proof).ok(),
            "round {round}"
        );
        for data in [
            ours.clone().unwrap_or_default(),
            r.damaged(&ours.unwrap_or_default()),
            random.clone(),
        ] {
            read_alike(
                "proof",
                round,
                &data,
                deserialize_nipopow_proof(&data).ok(),
                vm::deserialize_nipopow_proof(&data).ok(),
            );
        }
    }
}

#[test]
fn a_count_a_peer_gives_makes_room_for_no_more_than_the_bytes_it_sent() {
    // the most ids an inventory may claim, and the most modifiers 31 bits can say, with none of them sent
    let mut w = Writer::new();
    w.put_u8(101);
    w.put_u32(400);
    assert!(deserialize_inv(&w.result()).is_err());
    let mut w = Writer::new();
    w.put_u8(101);
    w.put_u32(i32::MAX as u32);
    assert!(deserialize_modifiers(&w.result()).is_err());
    // a modifier said to be as long as 31 bits can say, with a few bytes behind it
    let mut w = Writer::new();
    w.put_u8(101);
    w.put_u32(1);
    w.put_bytes(&[7; 32]);
    w.put_u32(i32::MAX as u32);
    w.put_bytes(&[1, 2, 3]);
    assert!(deserialize_modifiers(&w.result()).is_err());
    // the most a modifiers message may hold, to the byte, and one byte more: taken and refused by both alike
    for (len, taken) in [
        (2_048_576 * 4 - 5 - 36, true),
        (2_048_576 * 4 - 5 - 36 + 1, false),
    ] {
        let mut w = Writer::new();
        w.put_u8(101);
        w.put_u32(1);
        w.put_bytes(&[7; 32]);
        w.put_u32(len as u32);
        w.put_bytes(&vec![9u8; len]);
        let data = w.result();
        assert_eq!(deserialize_modifiers(&data).is_ok(), taken, "{len}");
        assert_eq!(
            vm::deserialize_modifiers(&data).is_ok(),
            taken,
            "{len}, by the vendored codec"
        );
    }
    // a proof likewise
    let mut w = Writer::new();
    w.put_u32(1_900_000);
    w.put_bytes(&[1, 2, 3]);
    assert!(deserialize_nipopow_proof(&w.result()).is_err());
}

// ---- a connection, against the vendored one over a socket of this machine ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn messages_cross_a_socket_between_this_connection_and_the_vendored_one_both_ways() {
    use tokio::io::AsyncWriteExt;
    let magic = [9, 8, 7, 6];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames: Vec<(u8, Vec<u8>)> = vec![
        (55, vec![1, 2, 3]),
        (1, vec![]),
        (33, (0..200_000u32).map(|i| i as u8).collect()),
        (65, vec![0xFF; 70_000]),
        (22, vec![4]),
    ];
    let sent = frames.clone();
    // the vendored side: sends them all, then reads them all back
    let theirs = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut conn = ergo_p2p::connection::Connection::new(stream, magic);
        for (code, payload) in &sent {
            conn.send(*code, payload.clone()).await.unwrap();
        }
        let mut back = vec![];
        for _ in 0..sent.len() {
            let f = conn.read_message().await.unwrap();
            back.push((f.code, f.payload));
        }
        back
    });
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut ours = Connection::new_with_buffer(stream, magic, Vec::new());
    let mut got = vec![];
    for _ in 0..frames.len() {
        let f = tokio::time::timeout(std::time::Duration::from_secs(20), ours.read_message())
            .await
            .unwrap()
            .unwrap();
        got.push((f.code, f.payload));
    }
    assert_eq!(
        got, frames,
        "what the vendored connection sent, as this one read it"
    );
    for (code, payload) in &frames {
        ours.send(*code, payload.clone()).await.unwrap();
    }
    assert_eq!(
        theirs.await.unwrap(),
        frames,
        "and what this one sent, as the vendored one read it"
    );

    // a frame that arrives a few bytes at a time, after bytes already in hand from the handshake's read; then one
    // whose checksum is not its body's, which ends the connection's use; then the peer going away
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let first = serialize_frame(
        &magic,
        &MessageFrame {
            code: 55,
            payload: vec![5; 40],
        },
    );
    let second = serialize_frame(
        &magic,
        &MessageFrame {
            code: 33,
            payload: (0..5000u32).map(|i| (i * 7) as u8).collect(),
        },
    );
    let mut bad = serialize_frame(
        &magic,
        &MessageFrame {
            code: 65,
            payload: vec![6; 20],
        },
    );
    bad[10] ^= 1;
    let (in_hand, rest) = (
        first[..7].to_vec(),
        [&first[7..], &second[..], &bad[..]].concat(),
    );
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        for piece in rest.chunks(13) {
            stream.write_all(piece).await.unwrap();
            stream.flush().await.unwrap();
        }
    });
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut ours = Connection::new_with_buffer(stream, magic, in_hand);
    let f = ours.read_message().await.unwrap();
    assert_eq!((f.code, f.payload), (55, vec![5; 40]));
    let f = ours.read_message().await.unwrap();
    assert_eq!((f.code, f.payload.len()), (33, 5000));
    assert!(matches!(
        ours.read_message().await,
        Err(ConnectionError::Frame(_))
    ));
    peer.await.unwrap();
    assert!(matches!(
        ours.read_message().await,
        Err(ConnectionError::Closed)
    ));
}
