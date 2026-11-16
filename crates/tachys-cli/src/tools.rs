//! Small commands: a token's reported name, a QR code as rows of bits, and what a peer sends unasked.

use std::time::Duration;

use crate::util::{arg, http};

/// tachys token --state DIR --node URL --id ID
pub(crate) fn token(args: &[String]) -> crate::out::Ended {
    // Look a token's name and decimals up once on an indexed node and keep them. What a node says about
    // a name is not proven, so the stored entry is marked as reported, unlike the names shipped with the wallet.
    let state: String = arg(args, "--state", "state".to_string())?;
    let node: String = crate::nodes::each(&crate::nodes::resolved(&arg(args, "--node", String::new())?, &state)?).into_iter().next().unwrap_or_default();
    let id: String = arg(args, "--id", String::new())?;
    let path = format!("{state}/tokens.json");
    let mut all: serde_json::Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::json!({}));
    if id.len() != 64 || node.is_empty() { return Err(crate::out::Fail::new(2, "usage: tachys token --state DIR --node URL|auto --id TOKENID".into())) }
    if all.get(&id).is_none() {
        let Ok(r) = http().get(&format!("{node}/blockchain/token/byId/{id}")).call() else { say!("token {id}: not found on {node}"); return Err(crate::out::Fail::said(1)) };
        let v: serde_json::Value = serde_json::from_str(&r.into_string().unwrap_or_default()).unwrap_or(serde_json::Value::Null);
        let name: String = v["name"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(24).collect();
        all[&id] = serde_json::json!({ "name": name, "decimals": v["decimals"].as_u64().unwrap_or(0), "reported_by": node });
        // kept so the node is asked once; if it cannot be kept the name is still shown, and asked for again next time
        let _ = std::fs::create_dir_all(&state);
        if let Ok(text) = serde_json::to_string_pretty(&all) { let _ = crate::util::write_whole(&path, text.as_bytes()); }
    }
    crate::out::set("id", &id); crate::out::set("name", &all[&id]["name"]); crate::out::set("decimals", &all[&id]["decimals"]); crate::out::set("reported_by", &all[&id]["reported_by"]);
    say!("{} {} decimals={}", id, all[&id]["name"].as_str().unwrap_or(""), all[&id]["decimals"]);
    Ok(())
}

/// tachys qr TEXT
pub(crate) fn qr(args: &[String]) -> crate::out::Ended {
    // a QR code as rows of 1 (dark) and 0 (light), for a UI to draw; keeps QR logic out of the UI layer
    let text = args.get(2).filter(|t| !t.starts_with("--")).cloned().unwrap_or_default();
    if text.is_empty() { return Err(crate::out::Fail::new(2, "usage: tachys qr TEXT".into())) }
    match qrcode::QrCode::new(text.as_bytes()) {
        Ok(code) => {
            let w = code.width();
            crate::out::set("rows", code.to_colors().chunks(w).map(|row| row.iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect::<String>()).collect::<Vec<_>>());
            for row in code.to_colors().chunks(w) { say!("{}", row.iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect::<String>()); }
        }
        Err(e) => { return Err(crate::out::Fail::new(1, format!("qr: {e:?}"))) }
    }
    Ok(())
}

/// tachys observe --peer ADDR [--secs N]
pub(crate) async fn observe(args: &[String]) -> crate::out::Ended {
    let peer: String = arg(args, "--peer", String::new())?;
    let secs: u64 = arg(args, "--secs", 25)?;
    match tachys_net::observe(&peer, secs, Duration::from_secs(60)).await {
        Err(e) => return Err(crate::out::Fail::new(if peer.is_empty() { 2 } else { 1 }, format!("observe {peer}: {e}"))),
        Ok(seen) => {
            let name = |c: u8| match c { 1 => "GetPeers", 2 => "Peers", 22 => "RequestModifier", 33 => "Modifiers", 55 => "Inv", 65 => "SyncInfo", 90 => "GetNipopowProof", 255 => "(connection closed)", _ => "other" };
            say!("frames received from {peer} in {secs} s:");
            for (c, (n, b)) in &seen { say!("  code {c:3} {:18} x{n}  {b} B", name(*c)); }
            say!("asked us for data (RequestModifier): {}; asked us for a proof (GetNipopowProof): {}", seen.get(&22).map_or(0, |x| x.0), seen.get(&90).map_or(0, |x| x.0));
        }
    }
    Ok(())
}
