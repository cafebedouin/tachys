//! The wallet as a library: `tachys::call` in a process that goes on living. No network.
use serde_json::Value;

/// A directory under the system's temporary one that is removed when the test is done with it, pass or fail.
struct Tmp(String);
impl std::ops::Deref for Tmp {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Display for Tmp {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn dir(name: &str) -> Tmp {
    let d = std::env::temp_dir().join(format!("tachys-lib-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d.to_string_lossy().to_string())
}
fn call(args: &[&str], input: &[&str]) -> (i32, Value, String) {
    let a = tachys::call(
        &std::iter::once("tachys")
            .chain(args.iter().copied())
            .map(String::from)
            .collect::<Vec<_>>(),
        &input.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    (
        a.code,
        serde_json::from_str(a.out.trim()).unwrap_or(Value::Null),
        a.err,
    )
}

#[test]
fn each_call_chooses_its_own_network_and_leaves_nothing_for_the_next() {
    let (main, test) = (dir("main"), dir("test"));
    // testnet, then mainnet, then testnet again, in one process: a network chosen once for the process could not do this
    let (code, t, _) = call(&["setup", "--state", &test, "--network", "testnet"], &[]);
    assert_eq!(
        (
            code,
            t["network"].as_str(),
            t["address"].as_str().map(|a| &a[..1])
        ),
        (0, Some("testnet"), Some("3"))
    );
    let (code, m, _) = call(&["setup", "--state", &main], &[]);
    assert_eq!(
        (
            code,
            m["network"].as_str(),
            m["address"].as_str().map(|a| &a[..1])
        ),
        (0, Some("mainnet"), Some("9"))
    );
    let (_, again, _) = call(&["address", "--state", &test, "--network", "testnet"], &[]);
    assert_eq!(again["address"], t["address"]);
    // the answer of one call holds nothing of the one before it
    let (code, j, err) = call(&["wallet", "address", "--state", &main, "--json"], &[]);
    assert_eq!(
        (
            code,
            &j["command"],
            j.get("created"),
            j.get("error"),
            j.get("network")
        ),
        (0, &"wallet address".into(), None, None, None)
    );
    assert!(
        err.contains("address 0: 9"),
        "its sentences are handed back, not printed: {err}"
    );
    // misuse is an answer too: the process goes on
    let (code, j, _) = call(&["wallet", "bogus", "--state", &main, "--json"], &[]);
    assert_eq!((code, &j["ok"]), (2, &Value::Bool(false)));
    let (code, j, _) = call(&["wallet", "send", "--nanoergs", "5", "--json"], &[]);
    assert!(code == 2 && j["error"].as_str().unwrap().contains("unknown option"));
    let (code, _, _) = call(&["wallet", "address", "--state", &main, "--json"], &[]);
    assert_eq!(
        code, 0,
        "and the call after a refusal is as good as the first"
    );
}

#[test]
fn a_password_and_a_phrase_are_handed_over_as_lines_not_read_from_stdin() {
    let st = dir("sealed");
    let (code, j, _) = call(
        &["wallet", "new", "--state", &st, "--pass-stdin", "--json"],
        &["correct horse"],
    );
    assert_eq!((code, &j["sealed"]), (0, &Value::Bool(true)));
    let on_disk = std::fs::read_to_string(format!("{st}/wallet.json")).unwrap();
    assert!(on_disk.contains("cipherText") && !on_disk.contains("mnemonic\""));
    // the wrong password is said to be so in a field; the right one opens the phrase
    let (code, j, _) = call(
        &[
            "wallet",
            "mnemonic",
            "--state",
            &st,
            "--pass-stdin",
            "--json",
        ],
        &["wrong"],
    );
    assert_eq!(
        (code, &j["bad_password"], j.get("mnemonic")),
        (1, &Value::Bool(true), None)
    );
    // a command that does not need the phrase goes on with it closed, and says the password was wrong
    let (code, j, _) = call(
        &[
            "wallet",
            "address",
            "--state",
            &st,
            "--pass-stdin",
            "--json",
        ],
        &["wrong"],
    );
    assert_eq!(
        (code, &j["bad_password"], &j["addresses"][0]),
        (0, &Value::Bool(true), &j_address(&st))
    );
    let (code, j, _) = call(
        &[
            "wallet",
            "mnemonic",
            "--state",
            &st,
            "--pass-stdin",
            "--json",
        ],
        &["correct horse"],
    );
    let phrase = j["mnemonic"].as_str().unwrap().to_string();
    assert_eq!((code, phrase.split(' ').count()), (0, 12));
    // the phrase restores the same wallet elsewhere: the password first, then the phrase, as two lines
    let other = dir("restored");
    let (_, d, _) = call(
        &["wallet", "derive", "--mnemonic-file", "-", "--json"],
        &[&phrase],
    );
    assert_eq!(d["address"], j_address(&st));
    let (_, r, _) = call(
        &[
            "wallet",
            "restore",
            "--state",
            &other,
            "--mnemonic-file",
            "-",
            "--embed",
            "--pass-stdin",
            "--json",
        ],
        &["another password", &phrase],
    );
    assert_eq!(
        (&r["restored"], &r["address"]),
        (&"new".into(), &d["address"]),
        "written, though with no node named its boxes are not looked for yet"
    );
    // no line to read is an empty password, never a wait on the process's stdin
    let (code, j, _) = call(
        &[
            "wallet",
            "mnemonic",
            "--state",
            &st,
            "--pass-stdin",
            "--json",
        ],
        &[],
    );
    assert!(code != 0 && j.get("mnemonic").is_none());
}
fn j_address(st: &str) -> Value {
    serde_json::from_str::<Value>(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
        .unwrap()["address"]
        .clone()
}

#[test]
fn calls_from_several_threads_run_at_once_each_on_its_own_network() {
    let dirs: Vec<(Tmp, &str)> = (0..6)
        .map(|i| {
            (
                dir(&format!("thread{i}")),
                if i % 2 == 0 { "mainnet" } else { "testnet" },
            )
        })
        .collect();
    // all six are inside a call before any goes on: with one network for the process, the last to choose would win
    let together = std::sync::Arc::new(std::sync::Barrier::new(dirs.len()));
    let handles: Vec<_> = dirs
        .into_iter()
        .map(|(d, net)| {
            let together = together.clone();
            std::thread::spawn(move || {
                together.wait();
                let mut all = vec![];
                for _ in 0..20 {
                    let (code, j, _) = call(&["setup", "--state", &d, "--network", net], &[]);
                    let (_, v, _) = call(&["version", "--network", net, "--json"], &[]);
                    all.push((
                        code,
                        j["network"].as_str().unwrap_or("").to_string(),
                        j["address"].as_str().unwrap_or("").to_string(),
                        v["network"].as_str().unwrap_or("").to_string(),
                        v["command"].as_str().unwrap_or("").to_string(),
                    ));
                }
                (net, all)
            })
        })
        .collect();
    for h in handles {
        let (asked, all) = h.join().unwrap();
        for (code, got, address, version_net, command) in all {
            assert_eq!(
                (code, got.as_str(), version_net.as_str(), command.as_str()),
                (0, asked, asked, "version"),
                "each got the network it asked for, and its own answer"
            );
            assert!(
                address.starts_with(if asked == "mainnet" { '9' } else { '3' }),
                "{asked}: {address}"
            );
        }
    }
}

#[test]
fn a_call_does_not_wait_for_one_that_is_still_running() {
    // a peer that takes the connection and says nothing holds `tips` until its deadline
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let mut held = vec![];
        for c in l.incoming() {
            held.push(c);
        }
    });
    let st = dir("slow");
    let seeds = format!("{st}/seeds.txt");
    std::fs::write(&seeds, &addr).unwrap();
    let (started, go) = std::sync::mpsc::channel();
    let slow = {
        let (st, seeds) = (st.to_string(), seeds.clone());
        std::thread::spawn(move || {
            started.send(()).unwrap();
            let t = std::time::Instant::now();
            let (code, _, _) = call(
                &[
                    "tips",
                    "--seeds",
                    &seeds,
                    "--peers",
                    "1",
                    "--tries",
                    "1",
                    "--quorum",
                    "1",
                    "--state",
                    &st,
                    "--network",
                    "testnet",
                    "--json",
                ],
                &[],
            );
            (code, t.elapsed())
        })
    };
    go.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let t = std::time::Instant::now();
    let (code, j, _) = call(
        &["qr", "an address to show while the sync runs", "--json"],
        &[],
    );
    let quick = t.elapsed();
    let (slow_code, slow_took) = slow.join().unwrap();
    assert_eq!(
        (code, &j["command"], j.get("verdict")),
        (0, &"qr".into(), None),
        "its own answer, with nothing of the other call's in it"
    );
    assert_ne!(slow_code, 0, "no tip: the peer said nothing");
    assert!(
        quick < slow_took / 2 && slow_took > std::time::Duration::from_secs(1),
        "the quick call took {quick:?} while the slow one took {slow_took:?}"
    );
}

/// Two commands that write one wallet take turns. They read the wallet file, changed it and wrote it back with nothing
/// between them: started together, one failed ("could not write"), and a slow one (a sync) that finished after a quick
/// one wrote the file as it had read it, the quick one's change gone.
#[test]
fn two_commands_that_write_one_wallet_take_turns_and_both_changes_are_kept() {
    for round in 0..12 {
        let st = dir(&format!("turns-{round}"));
        assert_eq!(
            call(
                &[
                    "wallet",
                    "new",
                    "--state",
                    &st,
                    "--network",
                    "testnet",
                    "--json"
                ],
                &[]
            )
            .0,
            0
        );
        let go = std::sync::Arc::new(std::sync::Barrier::new(3));
        let name = format!("n{round}");
        let run = |args: Vec<String>| {
            let go = go.clone();
            std::thread::spawn(move || {
                go.wait();
                call(&args.iter().map(String::as_str).collect::<Vec<_>>(), &[])
            })
        };
        let threads = [
            run([
                "wallet",
                "address",
                "--add",
                "--state",
                &st,
                "--network",
                "testnet",
                "--json",
            ]
            .map(String::from)
            .to_vec()),
            run([
                "wallet",
                "name",
                "--name",
                &name,
                "--state",
                &st,
                "--network",
                "testnet",
                "--json",
            ]
            .map(String::from)
            .to_vec()),
            run([
                "wallet",
                "address",
                "--add",
                "--state",
                &st,
                "--network",
                "testnet",
                "--json",
            ]
            .map(String::from)
            .to_vec()),
        ];
        for t in threads {
            let (code, j, err) = t.join().unwrap();
            assert_eq!(code, 0, "round {round}: {j} {err}");
        }
        let w: Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
                .unwrap();
        assert_eq!(
            (
                w["name"].as_str(),
                w["addresses"].as_u64(),
                w["address_list"].as_array().map(|a| a.len())
            ),
            (Some(name.as_str()), Some(3), Some(3)),
            "round {round}: the name and both addresses"
        );
    }
}

/// A command that only reads the wallet waits for no one: the words can be shown while another command has its turn.
#[test]
fn a_command_that_only_reads_the_wallet_does_not_wait_for_one_that_writes() {
    let st = dir("reads");
    assert_eq!(
        call(
            &[
                "wallet",
                "new",
                "--state",
                &st,
                "--network",
                "testnet",
                "--json"
            ],
            &[]
        )
        .0,
        0
    );
    // another program at work on this wallet: the lock every writing command takes, held here
    let held = std::fs::OpenOptions::new()
        .append(true)
        .open(format!("{st}/wallet.lock"))
        .unwrap();
    held.lock().unwrap();
    let t = std::time::Instant::now();
    let (code, j, _) = call(
        &[
            "wallet",
            "address",
            "--state",
            &st,
            "--network",
            "testnet",
            "--json",
        ],
        &[],
    );
    assert_eq!(
        (code, j["addresses"].as_array().map(|a| a.len())),
        (0, Some(1))
    );
    assert_eq!(
        call(
            &[
                "wallet",
                "mnemonic",
                "--state",
                &st,
                "--network",
                "testnet",
                "--json"
            ],
            &[]
        )
        .0,
        0
    );
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
    // and one that writes does wait: it goes through only once the other is done
    let (st2, waited) = (
        st.to_string(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let w2 = waited.clone();
    let writer = std::thread::spawn(move || {
        let r = call(
            &[
                "wallet",
                "name",
                "--name",
                "after",
                "--state",
                &st2,
                "--network",
                "testnet",
                "--json",
            ],
            &[],
        );
        (r.0, w2.load(std::sync::atomic::Ordering::SeqCst))
    });
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert!(!writer.is_finished(), "it is waiting its turn");
    waited.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(held);
    assert_eq!(writer.join().unwrap(), (0, true));
}

/// `wallet new` found no wallet and wrote one, with nothing held between: two at once on one directory could each find
/// none, and the second write over the first's seed phrase after the first had answered with its address.
#[test]
fn a_wallet_asked_for_twice_at_once_is_made_once_and_keeps_its_phrase() {
    for round in 0..6 {
        let st = dir(&format!("once-{round}"));
        let go = std::sync::Arc::new(std::sync::Barrier::new(4));
        let makers: Vec<_> = (0..4)
            .map(|_| {
                let (st, go) = (st.to_string(), go.clone());
                std::thread::spawn(move || {
                    go.wait();
                    call(
                        &[
                            "wallet",
                            "new",
                            "--state",
                            &st,
                            "--network",
                            "testnet",
                            "--json",
                        ],
                        &[],
                    )
                })
            })
            .collect();
        let answers: Vec<(i32, Value, String)> =
            makers.into_iter().map(|t| t.join().unwrap()).collect();
        let made: Vec<&Value> = answers.iter().filter(|a| a.0 == 0).map(|a| &a.1).collect();
        assert_eq!(made.len(), 1, "round {round}: {answers:?}");
        assert!(
            answers
                .iter()
                .filter(|a| a.0 != 0)
                .all(|a| a.0 == 2 && a.1["error"].as_str().unwrap().contains("already exists")),
            "round {round}: {answers:?}"
        );
        // the wallet on disk is the one that was answered for
        let w: Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
                .unwrap();
        assert_eq!(w["address"], made[0]["address"], "round {round}");
        let (code, j, _) = call(
            &[
                "wallet",
                "address",
                "--state",
                &st,
                "--network",
                "testnet",
                "--json",
            ],
            &[],
        );
        assert_eq!(
            (code, &j["addresses"][0]),
            (0, &made[0]["address"]),
            "and its phrase leads to that address"
        );
    }
}
