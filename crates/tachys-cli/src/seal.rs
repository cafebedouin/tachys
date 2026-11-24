//! The seed phrase: whether it is a valid one, and how it is sealed at rest.

use crate::util::{clip, hex, unhex};

/// What is wrong with a seed phrase, where that can be told (BIP-39: the word count, the word list, and the
/// checksum carried in its last bits). A mistyped phrase is otherwise a valid key to an empty wallet.
pub(crate) fn phrase_problem(phrase: &str) -> Option<String> {
    use ergo_lib::wallet::mnemonic_generator::{Language, WordList};
    use sha2::Digest;
    let words: Vec<&str> = phrase.split_whitespace().collect();
    if ![12, 15, 18, 21, 24].contains(&words.len()) {
        return Some(format!(
            "a seed phrase has 12, 15, 18, 21 or 24 words; this one has {}",
            words.len()
        ));
    }
    let lists = [
        Language::English,
        Language::ChineseSimplified,
        Language::ChineseTraditional,
        Language::French,
        Language::Italian,
        Language::Japanese,
        Language::Korean,
        Language::Spanish,
    ];
    let Some(list) = lists
        .into_iter()
        .map(|l| WordList(l).words())
        .find(|l| words.iter().all(|w| l.contains(w)))
    else {
        let english = WordList(Language::English).words();
        let bad = words
            .iter()
            .find(|w| !english.contains(w))
            .copied()
            .unwrap_or("");
        return Some(format!("\"{}\" is not a seed-phrase word", clip(bad, 24)));
    };
    // eleven bits a word; the last (words / 3) bits are the first bits of SHA-256 of the rest
    let mut bits: Vec<bool> = vec![];
    for w in &words {
        let i = list.iter().position(|x| x == w).unwrap_or(0);
        bits.extend((0..11).rev().map(|b| (i >> b) & 1 == 1));
    }
    let ent = bits.len() * 32 / 33;
    let entropy: Vec<u8> = bits[..ent]
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |a, b| (a << 1) | *b as u8))
        .collect();
    let hash = sha2::Sha256::digest(&entropy);
    if (0..bits.len() - ent).all(|i| bits[ent + i] == ((hash[i / 8] >> (7 - i % 8)) & 1 == 1)) {
        None
    } else {
        Some("the phrase's checksum fails: a word is wrong, missing or out of order".into())
    }
}

/// PBKDF2 rounds for a newly sealed phrase: the Ergo node's default (`ergo.wallet.secretStorage.encryption.c`).
pub(crate) const SEAL_ROUNDS: u32 = 128_000;

/// The seed phrase at rest, sealed the way ergo-wallet's JsonSecretStorage seals its secret: a key from the
/// password by PBKDF2-HMAC-SHA256 (128000 rounds, 256 bits, 32-byte salt), then AES-256-GCM with a 96-bit
/// nonce, written as the same JSON fields in hex. ergo-wallet's AES.scala cuts the cipher's output (ciphertext
/// then tag) after 16 bytes and names the first piece "authTag" and the rest "cipherText"; the same cut is made
/// here so the fields hold what they hold there. One difference: what is sealed is the phrase itself, not the
/// seed derived from it, so the owner can be shown the words again.
pub(crate) fn seal_phrase(phrase: &str, pass: &str) -> Option<serde_json::Value> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    let (mut salt, mut iv, mut key) = ([0u8; 32], [0u8; 12], [0u8; 32]);
    getrandom::getrandom(&mut salt).ok()?;
    getrandom::getrandom(&mut iv).ok()?;
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pass.as_bytes(), &salt, SEAL_ROUNDS, &mut key);
    let out = Aes256Gcm::new_from_slice(&key)
        .ok()
        .and_then(|c| c.encrypt(Nonce::from_slice(&iv), phrase.as_bytes()).ok());
    zeroize::Zeroize::zeroize(&mut key);
    let out = out?;
    Some(
        serde_json::json!({ "cipherText": hex(&out[16..]), "salt": hex(&salt), "iv": hex(&iv), "authTag": hex(&out[..16]),
        "cipherParams": { "prf": "HmacSHA256", "c": SEAL_ROUNDS, "dkLen": 256 }, "usePre1627KeyDerivation": false }),
    )
}

/// Opens what `seal_phrase` (or ergo-wallet, given the same plaintext) sealed. A wrong password fails the tag check.
pub(crate) fn open_phrase(s: &serde_json::Value, pass: &str) -> Result<String, String> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    let bytes = |k: &str| {
        unhex(s[k].as_str().unwrap_or(""))
            .filter(|b| !b.is_empty())
            .ok_or(format!("secret field {k} is missing or not hex"))
    };
    let p = &s["cipherParams"];
    if p["prf"].as_str() != Some("HmacSHA256") || p["dkLen"].as_u64() != Some(256) {
        return Err(
            "cipherParams this build cannot read (it reads HmacSHA256 with dkLen 256)".into(),
        );
    }
    let rounds = p["c"]
        .as_u64()
        .filter(|c| (1..=10_000_000).contains(c))
        .ok_or("cipherParams.c is missing or out of range")? as u32;
    let (salt, iv) = (bytes("salt")?, bytes("iv")?);
    if iv.len() != 12 {
        return Err("iv is not 96 bits".into());
    }
    let mut data = bytes("authTag")?;
    data.extend(bytes("cipherText")?);
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pass.as_bytes(), &salt, rounds, &mut key);
    let plain = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| e.to_string())
        .and_then(|c| {
            c.decrypt(Nonce::from_slice(&iv), data.as_ref())
                .map_err(|_| "bad wallet password".to_string())
        });
    zeroize::Zeroize::zeroize(&mut key);
    // (if these bytes are not text they are dropped unread; the phrase goes to the caller, who wipes it when done)
    String::from_utf8(plain?).map_err(|e| {
        let mut b = e.into_bytes();
        zeroize::Zeroize::zeroize(&mut b);
        "what was sealed is not a phrase".to_string()
    })
}
