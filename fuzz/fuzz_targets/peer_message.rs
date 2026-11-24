//! A message from a peer, through the peer protocol's decoders (the vendored codec): the first byte says which.
#![no_main]
use ergo_p2p::message::{deserialize_inv, deserialize_modifiers, deserialize_sync_info};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((which, body)) = data.split_first() else {
        return;
    };
    match which % 4 {
        0 => {
            let _ = deserialize_inv(body);
        }
        1 => {
            let _ = deserialize_modifiers(body);
        }
        2 => {
            let _ = deserialize_sync_info(body);
        }
        _ => {
            let _ = ergo_p2p::handshake::deserialize_handshake(body);
        }
    }
});
