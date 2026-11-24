//! Collects every option the commands read (`"--name"` as a whole string literal in the command code) into one
//! list, so that the check for a mistyped option (`util::unknown_option`) never falls behind the code: an option
//! is known the moment some command asks for it, and no list is kept by hand.
use std::collections::BTreeSet;

fn main() {
    println!("cargo:rerun-if-changed=src");
    let mut options: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir("src").expect("src") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if !name.ends_with(".rs") || name == "tests.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("source");
        // test modules name options that do not exist, on purpose
        let code = text.split("#[cfg(test)]\nmod tests {").next().unwrap_or("");
        let mut rest = code;
        while let Some(at) = rest.find("\"--") {
            let after = &rest[at + 1..];
            let end = after.find('"').unwrap_or(after.len());
            let word = &after[..end];
            if word.len() > 2
                && word[2..]
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                options.insert(word.to_string());
            }
            rest = &after[end.min(after.len())..];
        }
    }
    let list = options
        .iter()
        .map(|o| format!("{o:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("options.rs");
    std::fs::write(out, format!("/// Every option some command reads, gathered from the source by build.rs.\npub(crate) const OPTIONS: &[&str] = &[{list}];\n")).expect("write");
}
