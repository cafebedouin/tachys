//! A message from a peer, through the wallet's own decoders (`tachys_net::wire`) and through the vendored codec's
//! beside them: the first byte says which. A panic in either is a find, and so is the two disagreeing on whether
//! the bytes are a message, or on how much of them is the handshake.
#![no_main]
use ergo_p2p::{handshake as vh, message as vm};
use libfuzzer_sys::fuzz_target;
use tachys_net::wire;

fuzz_target!(|data: &[u8]| {
    let Some((which, body)) = data.split_first() else {
        return;
    };
    match which % 7 {
        0 => {
            let (ours, theirs) = (wire::deserialize_inv(body), vm::deserialize_inv(body));
            assert_eq!(
                ours.as_ref().ok().map(|d| (d.type_id, &d.ids)),
                theirs.as_ref().ok().map(|d| (d.type_id, &d.ids))
            );
        }
        1 => {
            let (ours, theirs) = (
                wire::deserialize_modifiers(body),
                vm::deserialize_modifiers(body),
            );
            assert_eq!(
                ours.as_ref().ok().map(|d| (d.type_id, &d.modifiers)),
                theirs.as_ref().ok().map(|d| (d.type_id, &d.modifiers))
            );
        }
        2 => {
            let ours = wire::deserialize_sync_info(body).ok().map(|s| match s {
                wire::SyncInfo::V1 { header_ids } => (1, header_ids.concat(), header_ids.len()),
                wire::SyncInfo::V2 { headers } => (2, headers.concat(), headers.len()),
            });
            let theirs = vm::deserialize_sync_info(body).ok().map(|s| match s {
                vm::SyncInfo::V1 { header_ids } => (1, header_ids.concat(), header_ids.len()),
                vm::SyncInfo::V2 { headers } => (2, headers.concat(), headers.len()),
            });
            assert_eq!(ours, theirs);
        }
        3 => {
            let (ours, theirs) = (
                wire::deserialize_handshake_with_consumed(body),
                vh::deserialize_handshake_with_consumed(body),
            );
            assert_eq!(
                ours.as_ref().ok().map(|(h, used)| (
                    h.time,
                    &h.peer_spec.agent_name,
                    &h.peer_spec.node_name,
                    h.peer_spec.features.len(),
                    *used
                )),
                theirs.as_ref().ok().map(|(h, used)| (
                    h.time,
                    &h.peer_spec.agent_name,
                    &h.peer_spec.node_name,
                    h.peer_spec.features.len(),
                    *used
                ))
            );
        }
        4 => assert_eq!(
            wire::deserialize_nipopow_proof(body).ok(),
            vm::deserialize_nipopow_proof(body).ok()
        ),
        5 => {
            let (ours, theirs) = (
                wire::deserialize_get_nipopow_proof(body),
                vm::deserialize_get_nipopow_proof(body),
            );
            assert_eq!(
                ours.ok().map(|d| (d.m, d.k, d.header_id_opt)),
                theirs.ok().map(|d| (d.m, d.k, d.header_id_opt))
            );
        }
        _ => {
            // a frame, on the network its first four bytes name
            if let Some(magic) = body.get(..4) {
                let _ = wire::deserialize_frame(&[magic[0], magic[1], magic[2], magic[3]], body);
            }
        }
    }
});
