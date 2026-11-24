//! The peer-protocol decoders of the vendored codec, given mutated messages and random bytes: refused, never a panic.
#[test]
fn a_mutated_peer_message_is_refused_without_a_panic() {
    use ergo_p2p::message::*;
    use ergo_p2p::types::{InvData, ModifiersData};
    let h = vec![7u8; 220];
    let seeds: Vec<(u8, Vec<u8>)> = vec![
        (
            0,
            serialize_inv(&InvData {
                type_id: 101,
                ids: vec![[1u8; 32], [2u8; 32], [3u8; 32]],
            })
            .unwrap(),
        ),
        (
            1,
            serialize_modifiers(&ModifiersData {
                type_id: 101,
                modifiers: vec![([1u8; 32], h.clone()), ([2u8; 32], vec![9u8; 300])],
            })
            .unwrap(),
        ),
        (
            2,
            serialize_sync_info(&SyncInfo::V2 {
                headers: vec![h.clone(), h.clone()],
            })
            .unwrap(),
        ),
    ];
    let mut x: u64 = 0xabcdef;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut panics = std::collections::BTreeMap::new();
    std::panic::set_hook(Box::new(|_| {}));
    for _ in 0..300_000 {
        let (k, s) = &seeds[(rnd() as usize) % seeds.len()];
        let mut v = s.clone();
        for _ in 0..(1 + rnd() % 4) {
            if v.is_empty() {
                break;
            }
            let i = (rnd() as usize) % v.len();
            match rnd() % 5 {
                0 => v[i] ^= 1 << (rnd() % 8),
                1 => v[i] = rnd() as u8,
                2 => v.insert(i, rnd() as u8),
                3 => {
                    v.remove(i);
                }
                _ => v[i] = [0, 0xff, 0x7f, 0x80][(rnd() % 4) as usize],
            }
        }
        let r = std::panic::catch_unwind(|| match k {
            0 => {
                let _ = deserialize_inv(&v);
            }
            1 => {
                let _ = deserialize_modifiers(&v);
            }
            _ => {
                let _ = deserialize_sync_info(&v);
            }
        });
        if r.is_err() {
            *panics.entry(*k).or_insert(0) += 1;
        }
    }
    for _ in 0..100_000 {
        let v: Vec<u8> = (0..rnd() % 600).map(|_| rnd() as u8).collect();
        if std::panic::catch_unwind(|| {
            let _ = ergo_p2p::handshake::deserialize_handshake(&v);
            let _ = deserialize_inv(&v);
            let _ = deserialize_modifiers(&v);
            let _ = deserialize_sync_info(&v);
        })
        .is_err()
        {
            *panics.entry(9).or_insert(0) += 1;
        }
    }
    let _ = std::panic::take_hook();
    assert!(panics.is_empty(), "panicked, by decoder: {panics:?}");
}
