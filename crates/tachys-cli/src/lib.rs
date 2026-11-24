//! tachys: an experimental light wallet for Ergo, as a library with one way in.
//!
//! `call` runs one command, given as the command line would give it, and hands back how it ended and what it
//! wrote. The `tachys` program is `cli`: the same thing with the process's own arguments, stdin and stdout.
//! A command never ends the process, reads no stdin of its own accord and leaves nothing behind for the next call:
//! the network, the answer and the peer protocol's magic are each call's own (`own.rs`), so calls from several
//! threads run at the same time.
#[macro_use]
mod out;
mod agent;
mod chain;
mod network;
mod nodes;
mod outbox;
mod own;
mod policy;
mod proofs;
mod seal;
mod serve;
mod sync;
mod tips;
mod tools;
mod util;
mod wallet;
mod walletfile;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod witness;

use crate::network::{Net, net, MAINNET, TESTNET};
use crate::util::arg;


/// The readers of bytes a node or a peer chooses, as the fuzz targets in `fuzz/` call them. Built only with
/// `--cfg fuzzing`, which `cargo fuzz` sets: no other build has this module.
#[cfg(fuzzing)]
pub mod fuzzing {
    /// A box proof walked and then verified, with no `catch_unwind`: a panic is a bound the walk missed.
    pub fn box_proof(root: &[u8], proof: &[u8], keys: &[[u8; 32]]) -> Option<Vec<Option<Vec<u8>>>> { crate::proofs::lookup_unguarded(root, proof, keys) }
}

/// How a call ended and what it wrote. With `--json` among its arguments, `out` is the one JSON object.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// 0: done; 1: refused, failed or not yet so; 2: misuse; 70: the command panicked (a fault in this library)
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Runs one command. `args` are as on the command line, the program's name first (`["tachys", "balance", "--state",
/// dir]`); `input` are the lines the command may read where the program would read stdin (the password, then a seed
/// phrase). Nothing is written to the process's stdout or stderr and the process is never ended. Calls from several
/// threads run at the same time, each with its own network and answer; two that write one state directory are two
/// runs of the program on it. Not to be called from inside an async runtime: it runs its own.
pub fn call(args: &[String], input: &[String]) -> Answer {
    let (code, out, err) = run(args, true, Some(input.to_vec()));
    Answer { code, out, err }
}

/// The `tachys` program: one call with the process's arguments, reading stdin and writing stdout and stderr.
/// Returns the exit code.
pub fn cli() -> i32 { run(&std::env::args().collect::<Vec<_>>(), false, None).0 }

/// One call: how it ended and, if it was captured, what it wrote.
fn run(args: &[String], capture: bool, input: Option<Vec<String>>) -> (i32, String, String) {
    let own = own::Own::new();
    let _mine = own::enter(&own);
    out::start(args, capture, input);
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // every thread of the runtime works for this call: what it says and the network it is on are this call's
        let theirs = own.clone();
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().on_thread_start(move || own::join(&theirs)).build().map_err(|e| out::Fail::new(1, format!("runtime: {e}")))?;
        rt.block_on(async { setup(args)?; dispatch(args).await })
    }));
    // The one place a call ends. A command hands back how it went; with `--json` its one object is written here.
    let code = match ended {
        Ok(Ok(())) => 0,
        Ok(Err(f)) => { if let Some(m) = f.msg { complain!("{m}"); } f.code }
        Err(_) => { complain!("the command panicked: a fault in tachys, not in what was asked"); 70 }
    };
    // An agent command answers with one JSON object whatever happens: if it stopped before writing its own (an option
    // that is not one, a network that is not one, a panic), the reason is written here in the same shape.
    if args.get(1).is_some_and(|c| agent::COMMANDS.contains(&c.as_str())) && !out::answered() {
        let why = out::answer(code)["error"].as_str().unwrap_or("failed").to_string();
        out::write_out(&serde_json::json!({ "ok": false, "v": out::V, "code": if code == 2 { "misuse" } else { "failed" }, "refused": why }).to_string());
        out::own_answer();
    }
    out::finish(code);
    out::forget_input();
    let (out, err) = out::written();
    (code, out, err)
}

/// Sets the call up from its arguments: the options are ones some command reads, and the network is chosen.
fn setup(args: &[String]) -> out::Ended {
    // `tachys qr TEXT` takes any text; everywhere else a word that looks like an option has to be one
    if args.get(1).map(|s| s.as_str()) != Some("qr") { if let Some(o) = util::unknown_option(args) { return Err(crate::out::Fail::new(2, format!("unknown option {o}"))) } }
    // every call says which network it is on: nothing is left over from the call before
    network::set_net(match arg(args, "--network", "mainnet".to_string())?.as_str() {
        "mainnet" => &MAINNET, "testnet" => &TESTNET,
        // a slip here must not land a wallet on the network with real money
        other => return Err(out::Fail::new(2, format!("--network is mainnet or testnet, not \"{}\"", util::clip(other, 20)))) });
    // a private network (the rig): constants come from a JSON file written when the network is set up
    let net_file: String = arg(args, "--net-file", String::new())?;
    if !net_file.is_empty() {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&net_file).unwrap_or_default()).unwrap_or(serde_json::Value::Null);
        let leak = |x: String| -> &'static str { Box::leak(x.into_boxed_str()) };
        let m: Vec<u8> = v["magic"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64().map(|b| b as u8)).collect()).unwrap_or_default();
        if m.len() != 4 { return Err(crate::out::Fail::new(2, format!("--net-file {net_file}: unreadable"))) }
        let n = Net { name: leak(v["name"].as_str().unwrap_or("private").to_string()), magic: [m[0], m[1], m[2], m[3]],
            genesis: leak(v["genesis"].as_str().unwrap_or("").to_string()), checkpoint_json: leak(v["checkpoint"].to_string()),
            floor_divisor: v["floor_divisor"].as_u64().unwrap_or(1 << 20) as u32, v2_activation_height: v["v2_activation_height"].as_u64().unwrap_or(0) as u32,
            block_interval_ms: v["block_interval_ms"].as_u64().unwrap_or(1000),
            epoch_length: v["epoch_length"].as_u64().filter(|&e| (1..=u32::MAX as u64).contains(&e)).unwrap_or(128) as u32, seeds: leak(v["seeds"].as_str().unwrap_or("").to_string()), testnet: true, fee_tree: leak(v["fee_tree"].as_str().unwrap_or("").to_string()) };
        network::set_net(Box::leak(Box::new(n)));
    }
    if !network::checkpoint_readable() { return Err(out::Fail::new(2, format!("--net-file {net_file}: its checkpoint is not a header"))) }
    tachys_net::set_network_magic(net().magic);
    match arg(args, "--declare", "light".to_string())?.as_str() {
        "nothing" => tachys_net::set_declared(tachys_net::Declared::Nothing),
        "light" => tachys_net::set_declared(tachys_net::Declared::Light),
        "full-claim-control" => { if net().name != "devnet" { return Err(crate::out::Fail::new(2, "--declare full-claim-control is for our own devnet only".to_string())) } tachys_net::set_declared(tachys_net::Declared::FullClaimForControlOnly) }
        other => return Err(out::Fail::new(2, format!("--declare is light or nothing, not \"{}\"", util::clip(other, 20)))),
    }
    Ok(())
}

/// Hands the command to its module.
pub(crate) async fn dispatch(args: &[String]) -> out::Ended {
    if args.get(1).map(|s| s.as_str()) == Some("serve") { return serve::serve(args).await }
    if args.get(1).is_some_and(|c| agent::COMMANDS.contains(&c.as_str())) { return agent::agent(args) }
    if args.get(1).map(|s| s.as_str()) == Some("version") {
        let (cp, age) = (network::checkpoint_header(), network::checkpoint_age_days());
        out::set("version", env!("CARGO_PKG_VERSION")); out::set("network", net().name); out::set("checkpoint_height", cp.height); out::set("checkpoint_age_days", age);
        say!("tachys {} ({}); checkpoint at height {}, {age} days old{}", env!("CARGO_PKG_VERSION"), net().name, cp.height,
            if age > network::CHECKPOINT_OLD_DAYS { ": past the sixty days a build is good for; take a newer one" } else { "" });
        return Ok(());
    }
    if args.get(1).map(|s| s.as_str()) == Some("nodes") { return nodes::nodes(args) }
    if args.get(1).map(|s| s.as_str()) == Some("token") { return tools::token(args) }
    if args.get(1).map(|s| s.as_str()) == Some("qr") { return tools::qr(args) }
    if args.get(1).map(|s| s.as_str()) == Some("handoff") { return outbox::handoff(args).await }
    if args.get(1).map(|s| s.as_str()) == Some("outbox") { return outbox::outbox(args).await }
    if args.get(1).map(|s| s.as_str()) == Some("observe") { return tools::observe(args).await }
    if args.get(1).map(|s| s.as_str()) == Some("check") { return sync::check(&args[2..].iter().filter(|a| !a.starts_with("--")).cloned().collect::<Vec<_>>()) }
    if args.get(1).map(|s| s.as_str()) == Some("tips") { return tips::tips(args).await }
    if args.get(1).map(|s| s.as_str()) == Some("included") { return proofs::included(args) }
    if args.get(1).map(|s| s.as_str()) == Some("boxes") { return proofs::boxes(args) }
    if args.get(1).map(|s| s.as_str()) == Some("wallet") { return wallet::wallet(args) }
    if args.get(1).map(|s| s.as_str()) != Some("sync") {
        return Err(out::Fail::new(2, format!("{}usage: tachys COMMAND [--state DIR] [--network mainnet|testnet] [--json] ...\n  for programs (one JSON object each; `tachys recipes` explains): {}\n  the wallet: wallet new|restore|watch|status|receive|send|request|promise|consolidate|burn|address|name|encrypt|mnemonic|pubkey|babel-offer|babel-list\n  the chain: tips, nodes, boxes, included, sync, check\n  carrying payments: serve, handoff, outbox\n  small things: token, qr, observe\nSee README.md and docs/json.md.",
            match args.get(1) { Some(c) => format!("tachys: no such command \"{}\"\n", util::clip(c, 30)), None => String::new() }, agent::COMMANDS.join(", "))));
    }
    sync::sync(args).await
}
