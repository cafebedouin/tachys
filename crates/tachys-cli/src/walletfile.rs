//! The wallet file (`<state>/wallet.json`): what is in it, by name and type, and which version of that it is.
//!
//! The app reads this file too. A field renamed or retyped on one side used to show up on the other as a wallet that
//! was silently not there; now the file says which layout it holds, and a layout this build does not know, or a
//! field of the wrong type, is refused in words and the file is left as it is.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// The layout this build writes. A file with no `version` is layout 0: the same fields, from before the number was
/// kept; it is read as it is and gets the number the next time it is written. A higher number than this is a
/// newer build's file and is not read.
pub(crate) const VERSION: u64 = 1;

/// `null` where a value is expected reads as the empty value (older files wrote `null` for "none").
fn or_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(
    d: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct WalletFile {
    #[serde(default)]
    pub(crate) version: u64,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) name: String,
    /// The first address. Written beside a sealed phrase so a wallet can watch and receive without its password.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) address: String,
    /// Every address of the phrase in use, the first one first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) address_list: Option<Vec<String>>,
    /// How many addresses of the phrase are in use (absent: one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) addresses: Option<u64>,
    /// The height the wallet was made at: nothing before it is ever looked for.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) birthday: u64,
    /// A read-only wallet: an address and no key.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "is_false"
    )]
    pub(crate) watch: bool,
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "is_false"
    )]
    pub(crate) restored: bool,
    /// A restored wallet whose twenty addresses have not been looked over yet.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "is_false"
    )]
    pub(crate) scan_addresses: bool,
    /// The nodes are to be asked again what the addresses hold: another wallet of this phrase may have moved coins.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "is_false"
    )]
    pub(crate) refresh: bool,
    /// The seed phrase in plain text (no password was given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mnemonic: Option<String>,
    /// A file that holds the seed phrase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mnemonic_file: Option<String>,
    /// The seed phrase sealed with a password (`seal.rs`), in ergo-wallet's JSON fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) secret: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) warning: Option<String>,
    /// The boxes this wallet tracks, by box id. Always written, empty or not.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) boxes: BTreeMap<String, TrackedBox>,
    /// Boxes this wallet expects to appear: change from its own payments, and boxes promised to it.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) expect: Vec<String>,
    /// Boxes promised to this wallet (it held the payment) whose payment can never confirm: a coin it spends is gone,
    /// or it was refused for good. Newest last; the last 64.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) void: Vec<String>,
    /// Boxes this wallet had confirmed that left the chain with the block that confirmed them (a reorganisation). If
    /// one is proven again it is not new: it is the same payment back. Newest last; the last 64.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) undone: Vec<String>,
    /// Payments sent and not yet settled.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) pending: Vec<Pending>,
    /// The last twenty payments that were settled, newest first, and any older one given up as "not included" whose
    /// coins the wallet still tracks.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) done: Vec<Done>,
    /// Anything else in the file is kept as it was found.
    #[serde(flatten)]
    pub(crate) other: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Asset {
    pub(crate) id: String,
    pub(crate) amount: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct TrackedBox {
    /// nanoERG
    pub(crate) value: u64,
    /// how many kinds of token it holds
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) tokens: usize,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) assets: Vec<Asset>,
    /// the height of the last proof that showed it unspent
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) proven_at: u64,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) first_proven: u64,
    /// the first height at or below an agreed tip at which it was proven; `None` while only seen above one
    #[serde(default)]
    pub(crate) confirmed_at: Option<u64>,
    /// the id of the header at `confirmed_at` the proof was read against: if the chain the peers agree on later has
    /// another header there, the block that confirmed it is gone
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub(crate) confirmed_in: String,
    /// the box itself, in hex; it must hash to the id it is filed under to be used
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) bytes: String,
    #[serde(flatten)]
    pub(crate) other: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Pending {
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) tx: String,
    /// every box it spends
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) inputs: Vec<String>,
    /// one box it creates (its first)
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) watch: String,
    /// the signed transaction as the node's JSON, to send again if a node dropped it
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) json: String,
    /// the height of the first proof that looked for it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first_look: Option<u64>,
    #[serde(flatten)]
    pub(crate) other: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Done {
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) tx: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) watch: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) inputs: Vec<String>,
    /// "confirmed", "coins spent" or "not included" (`wallet::payment_outcome`); older files may hold "spent elsewhere"
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) result: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) seen_at: u64,
    #[serde(flatten)]
    pub(crate) other: BTreeMap<String, serde_json::Value>,
}

impl WalletFile {
    pub(crate) fn new(name: String, address: String) -> Self {
        WalletFile {
            version: VERSION,
            name,
            address,
            ..Default::default()
        }
    }
    pub(crate) fn nanoerg(&self) -> u64 {
        self.boxes
            .values()
            .fold(0u64, |a, b| a.saturating_add(b.value))
    }
    /// What the wallet holds of each token in boxes confirmed under an agreed tip, by id.
    pub(crate) fn confirmed_tokens(&self) -> BTreeMap<String, u64> {
        let mut t: BTreeMap<String, u64> = BTreeMap::new();
        for a in self
            .boxes
            .values()
            .filter(|b| b.confirmed_at.is_some())
            .flat_map(|b| b.assets.iter())
        {
            let n = t.entry(a.id.clone()).or_default();
            *n = n.saturating_add(a.amount);
        }
        t
    }
    /// What the wallet holds of each token, by id.
    pub(crate) fn tokens(&self) -> BTreeMap<String, u64> {
        let mut t: BTreeMap<String, u64> = BTreeMap::new();
        for a in self.boxes.values().flat_map(|b| b.assets.iter()) {
            let n = t.entry(a.id.clone()).or_default();
            *n = n.saturating_add(a.amount);
        }
        t
    }
}

/// Reads the text of a wallet file. An `Err` says why in words fit to show; the caller leaves the file alone.
pub(crate) fn parse(text: &str) -> Result<WalletFile, String> {
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| "it is not whole JSON (damaged, or cut short)".to_string())?;
    if !v.is_object() {
        return Err("it is not a wallet file".into());
    }
    match &v["version"] {
        serde_json::Value::Null => {}
        n => match n.as_u64() {
            Some(n) if n <= VERSION => {}
            Some(n) => return Err(format!("it was written by a newer tachys (wallet file version {n}; this build reads up to {VERSION})")),
            None => return Err("its version is not a number".into()),
        },
    }
    // the field is named by its path in the file (`boxes.aa.value`): serde_json's own error does not say which it is
    serde_path_to_error::deserialize(v).map_err(|e| {
        format!(
            "a field is not what this build expects (`{}`: {})",
            e.path(),
            e.inner()
        )
    })
}

/// The text to write: always this build's version.
pub(crate) fn text(w: &WalletFile) -> Result<String, String> {
    let mut w = w.clone();
    w.version = VERSION;
    serde_json::to_string_pretty(&w).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `vectors/app/wallet-file.json` is a wallet file as this build writes it, which the app's tests read
    /// (`ViewTest.kt`): the balance on a wallet's card, its tokens and what Send lets be sent are the app's own reading
    /// of this file, and this holds the file the app is tested on to the layout and the sums of the core.
    #[test]
    fn the_wallet_file_the_app_is_tested_on_is_one_this_build_writes() {
        let id = |c: char| c.to_string().repeat(64);
        let boxed = |value: u64,
                     confirmed_at: Option<u64>,
                     proven_at: u64,
                     assets: &[(char, u64)]| TrackedBox {
            value,
            tokens: assets.len(),
            proven_at,
            first_proven: proven_at,
            confirmed_at,
            assets: assets
                .iter()
                .map(|(t, amount)| Asset {
                    id: id(*t),
                    amount: *amount,
                })
                .collect(),
            ..Default::default()
        };
        let mut w = WalletFile::new("kept for the app's tests".into(), "9abc".into());
        w.boxes
            .insert(id('1'), boxed(1_500_000_000, Some(100), 105, &[('a', 700)]));
        w.boxes.insert(
            id('2'),
            boxed(
                2_000_000_001,
                Some(101),
                106,
                &[('a', 300), ('b', i64::MAX as u64)],
            ),
        );
        // proven only in a block one node has shown: beside the balance, not in it
        w.boxes
            .insert(id('3'), boxed(900_000_000, None, 107, &[('a', 5)]));
        w.boxes.insert(
            id('4'),
            boxed(1_000_000, Some(102), 106, &[('b', i64::MAX as u64)]),
        );
        w.expect = vec![id('e')];
        w.void = vec![id('f')];
        let seen: u64 = w
            .boxes
            .values()
            .filter(|b| b.confirmed_at.is_none())
            .map(|b| b.value)
            .sum();
        assert_eq!((w.nanoerg() - seen, seen), (3_501_000_001, 900_000_000));
        assert_eq!(
            w.confirmed_tokens().into_iter().collect::<Vec<_>>(),
            vec![(id('a'), 1_000), (id('b'), 2 * (i64::MAX as u64))]
        );
        let file = format!(
            "{}/../../vectors/app/wallet-file.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let written = text(&w).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap_or_default().trim(),
            written.trim(),
            "{file} is not what this build writes:\n{written}"
        );
    }

    /// A file as builds before the version number wrote it, with everything a used wallet accumulates.
    const OLD: &str = r#"{ "name": "Savings", "address": "9abc", "address_list": ["9abc", "9def"], "addresses": 2, "birthday": 1900000, "restored": true,
        "secret": { "cipherText": "00", "salt": "11", "iv": "22", "authTag": "33", "cipherParams": { "prf": "HmacSHA256", "c": 128000, "dkLen": 256 }, "usePre1627KeyDerivation": false },
        "boxes": { "aa": { "value": 5000000, "tokens": 1, "assets": [{ "id": "tt", "amount": 7 }], "proven_at": 1917000, "first_proven": 1916990, "confirmed_at": null, "bytes": "c0ffee" },
                   "bb": { "value": 1000000, "bytes": "beef" } },
        "expect": ["cc"],
        "pending": [{ "tx": "p1", "inputs": ["i1", "i2"], "watch": "w1", "ours": [], "json": "{}", "first_look": 1917001 }],
        "done": [{ "tx": "d1", "watch": null, "inputs": ["i3"], "result": "confirmed", "seen_at": 1916000 }],
        "something_a_later_build_added": { "kept": true } }"#;

    #[test]
    fn a_file_from_before_the_version_number_reads_and_is_given_one_when_written() {
        let w = parse(OLD).unwrap();
        assert_eq!(
            (
                w.version,
                w.name.as_str(),
                w.address.as_str(),
                w.addresses,
                w.birthday,
                w.restored,
                w.watch
            ),
            (0, "Savings", "9abc", Some(2), 1_900_000, true, false)
        );
        assert_eq!(
            w.address_list,
            Some(vec!["9abc".to_string(), "9def".to_string()])
        );
        assert!(w.secret.as_ref().unwrap()["cipherParams"]["c"] == 128000 && w.mnemonic.is_none());
        assert_eq!(
            (w.nanoerg(), w.tokens().get("tt").copied()),
            (6_000_000, Some(7))
        );
        assert_eq!(
            (
                w.boxes["aa"].confirmed_at,
                w.boxes["bb"].proven_at,
                w.boxes["bb"].bytes.as_str()
            ),
            (None, 0, "beef"),
            "what an old file left out reads as nothing"
        );
        assert_eq!(
            (
                w.pending[0].inputs.len(),
                w.pending[0].first_look,
                w.done[0].watch.as_str(),
                w.done[0].seen_at
            ),
            (2, Some(1_917_001), "", 1_916_000)
        );
        // written back: this build's version, and nothing that was there is lost, known to this build or not
        let again: serde_json::Value = serde_json::from_str(&text(&w).unwrap()).unwrap();
        assert_eq!(again["version"], VERSION);
        let was: serde_json::Value = serde_json::from_str(OLD).unwrap();
        for k in [
            "name",
            "address",
            "address_list",
            "addresses",
            "birthday",
            "restored",
            "secret",
            "expect",
            "something_a_later_build_added",
        ] {
            assert_eq!(again[k], was[k], "{k}");
        }
        assert_eq!(again["boxes"]["aa"], was["boxes"]["aa"]);
        assert_eq!(
            again["pending"][0], was["pending"][0],
            "a field this build does not use (ours) is kept too"
        );
        assert_eq!(
            (
                again["done"][0]["tx"].clone(),
                again["done"][0]["result"].clone()
            ),
            (
                was["done"][0]["tx"].clone(),
                was["done"][0]["result"].clone()
            )
        );
        assert_eq!(
            parse(&text(&w).unwrap()).unwrap(),
            WalletFile {
                version: VERSION,
                ..w
            },
            "and it reads back the same"
        );
    }

    #[test]
    fn what_the_app_reads_is_always_there() {
        // the app needs `boxes` (an object), `address`, and each box's `value`; a new wallet has them before it holds anything
        let v: serde_json::Value =
            serde_json::from_str(&text(&WalletFile::new("Wallet".into(), "9abc".into())).unwrap())
                .unwrap();
        assert!(v["boxes"].is_object() && v["address"] == "9abc" && v["version"] == VERSION);
        assert!(
            v.get("watch").is_none() && v.get("mnemonic").is_none() && v.get("pending").is_none(),
            "nothing that is not so is written"
        );
        let watch: serde_json::Value = serde_json::from_str(
            &text(&WalletFile {
                watch: true,
                ..WalletFile::new("W".into(), "9abc".into())
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(watch["watch"], true);
    }

    #[test]
    fn a_file_this_build_cannot_read_is_refused_in_words() {
        assert!(parse(r#"{ "version": 2, "address": "9abc", "boxes": {} }"#)
            .unwrap_err()
            .contains("newer tachys"));
        assert!(parse(r#"{ "version": "one", "address": "9abc" }"#)
            .unwrap_err()
            .contains("version is not a number"));
        assert!(parse(&OLD[..OLD.len() / 2])
            .unwrap_err()
            .contains("cut short"));
        assert!(parse("[]").unwrap_err().contains("not a wallet file"));
        // a field of the wrong type is named
        for (bad, field) in [
            (r#"{ "address": "9abc", "boxes": [] }"#, "boxes"),
            (r#"{ "address": 5 }"#, "address"),
            (
                r#"{ "address": "9abc", "boxes": { "aa": { "value": "lots" } } }"#,
                "value",
            ),
            (
                r#"{ "address": "9abc", "boxes": { "aa": { "bytes": "00" } } }"#,
                "value",
            ),
            (
                r#"{ "address": "9abc", "pending": [{ "inputs": "i1" }] }"#,
                "inputs",
            ),
            (r#"{ "address": "9abc", "addresses": -1 }"#, "addresses"),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains("not what this build expects"), "{bad}: {e}");
            // by its path (`boxes.aa.value`: ...), or, when it is missing, in the reason (`boxes.aa`: missing field `value`)
            assert!(
                e.contains(&format!("{field}`")),
                "{bad}: the field is named: {e}"
            );
        }
        assert!(
            parse(r#"{ "version": 1, "address": "9abc" }"#).is_ok(),
            "this build's own version, with nothing else, is a wallet with no boxes"
        );
    }
}
