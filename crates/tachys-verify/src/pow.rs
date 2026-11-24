//! The Autolykos v2 proof-of-work hit of a header, and whether it meets the header's own difficulty.
//!
//! The same computation as the library's `Header::check_pow` and `AutolykosPowScheme::pow_hit`, but for one step.
//! At the sigma-rust revision this build uses, `AutolykosPowScheme::gen_indexes` reads each of a header's 32 table
//! indexes as `.to_u32_digits().1[0]`, and that panics when an index is 0, since zero has no digits
//! (<https://github.com/ergoplatform/sigma-rust/issues/964>). It runs before the hit is compared with the target, so no
//! work is needed to make such a header, only a scan of nonces (about 32 in N of them, N being the table size). An
//! index of 0 is simply the table's element 0, as the reference node computes it (`mod(N).toInt`), and honest headers
//! have one about 32 times in N. So the index is computed here, and every other step is the library's own.
use ergo_chain_types::autolykos_pow_scheme::{
    decode_compact_bits, order_bigint, AutolykosPowScheme, AutolykosPowSchemeError,
};
use ergo_chain_types::{blake2b256_hash, Header};
use num_bigint::{BigUint, ToBigInt};
use num_traits::Zero;

/// The `k` table indexes a seed picks, each in `[0, big_n)`: index `i` is the four bytes of the seed (wrapped
/// round to its first three) from byte `i`, read big-endian, modulo `big_n`. 0 included.
pub fn indexes(seed: &[u8; 32], big_n: u32, k: usize) -> Vec<u32> {
    let mut ext = seed.to_vec();
    ext.extend_from_slice(&seed[..3]);
    (0..k)
        .map(|i| u32::from_be_bytes([ext[i], ext[i + 1], ext[i + 2], ext[i + 3]]) % big_n)
        .collect()
}

/// The table indexes a version 2 header's solution picks (32 of them).
pub fn header_indexes(h: &Header) -> Result<Vec<u32>, AutolykosPowSchemeError> {
    let scheme = AutolykosPowScheme::default();
    let msg = blake2b256_hash(&h.serialize_without_pow()?).0;
    let big_n = scheme.calc_big_n(h.version, h.height);
    let seed = scheme.calc_seed_v2(
        big_n,
        &msg,
        &h.autolykos_solution.nonce,
        &h.height.to_be_bytes(),
    )?;
    Ok(indexes(&seed, big_n, 32))
}

/// The proof-of-work hit of a header: the lower, the more work it took to find. `AutolykosPowScheme::pow_hit`, with
/// an index of 0 read as 0.
pub fn pow_hit(h: &Header) -> Result<BigUint, AutolykosPowSchemeError> {
    if h.version == 1 {
        return h
            .autolykos_solution
            .pow_distance
            .clone()
            .ok_or(AutolykosPowSchemeError::MissingPowDistanceParameter);
    }
    let scheme = AutolykosPowScheme::default();
    let height = h.height.to_be_bytes();
    let big_m = scheme.calc_big_m();
    // f2: the sum of the elements the indexes pick, each the hash of (index, height, M) without its first byte
    let sum: BigUint = header_indexes(h)?
        .into_iter()
        .map(|i| {
            let mut e = i.to_be_bytes().to_vec();
            e.extend_from_slice(&height);
            e.extend_from_slice(&big_m);
            BigUint::from_bytes_be(&blake2b256_hash(&e).0[1..])
        })
        .sum();
    // as 32 bytes, big-endian, padded on the left (32 elements of 31 bytes sum to less than 2^253)
    let bytes = sum.to_bytes_be();
    if bytes.len() > 32 {
        return Err(AutolykosPowSchemeError::BigIntToByteArrayError);
    }
    let mut array = [0u8; 32];
    array[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(BigUint::from_bytes_be(&blake2b256_hash(&array).0))
}

/// `Header::check_pow`, the same answer for every header it answers, and one for a header with an index of 0 in
/// place of a panic: does the hit fall below the target its declared difficulty sets? Version 1 is not supported, and
/// a difficulty of zero is an error, as there.
pub fn pow_ok(h: &Header) -> Result<bool, AutolykosPowSchemeError> {
    if h.version == 1 {
        return Err(AutolykosPowSchemeError::Unsupported);
    }
    let hit = pow_hit(h)?;
    let difficulty = decode_compact_bits(h.n_bits);
    if difficulty.is_zero() {
        return Err(AutolykosPowSchemeError::OutOfBounds);
    }
    let target = order_bigint() / difficulty;
    Ok(hit.to_bigint().is_some_and(|hit| hit < target))
}
