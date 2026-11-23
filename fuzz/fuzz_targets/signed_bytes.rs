//! Bytes that reach the signing library's decoders from a node's pool, a carried payment or a page's signing
//! request: a transaction, a box, a reduced transaction. The first byte says which.
#![no_main]
use ergo_lib::chain::transaction::reduced::ReducedTransaction;
use ergo_lib::chain::transaction::Transaction;
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::serialization::SigmaSerializable;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((which, body)) = data.split_first() else { return };
    match which % 3 {
        0 => { let _ = Transaction::sigma_parse_bytes(body); }
        1 => { let _ = ErgoBox::sigma_parse_bytes(body); }
        _ => { let _ = ReducedTransaction::sigma_parse_bytes(body); }
    }
});
