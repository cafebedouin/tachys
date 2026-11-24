//! The C interface, called as C would call it. No network.
use std::ffi::{c_char, CStr, CString};
use tachys_c::{tachys_answer, tachys_call, tachys_free};

/// A directory under the system's temporary one that is removed when the test is done with it, pass or fail.
struct Tmp(String);
impl Drop for Tmp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn dir(name: &str) -> Tmp {
    let d = std::env::temp_dir().join(format!("tachys-c-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d.to_string_lossy().to_string())
}
/// One call through the C interface: the code, the JSON object and the sentences.
fn call(args: &[&str], input: &[&str]) -> (i32, serde_json::Value, String) {
    let own = |l: &[&str]| l.iter().map(|s| CString::new(*s).unwrap()).collect::<Vec<_>>();
    let (args, input) = (own(&[&["tachys"], args].concat()), own(input));
    let ptrs = |l: &[CString]| l.iter().map(|s| s.as_ptr()).collect::<Vec<*const c_char>>();
    let (a, i) = (ptrs(&args), ptrs(&input));
    // as the header allows: no list at all when there are no input lines
    let answer: *mut tachys_answer = unsafe { tachys_call(a.as_ptr(), a.len(), if i.is_empty() { std::ptr::null() } else { i.as_ptr() }, i.len()) };
    assert!(!answer.is_null());
    let (code, out, err) = unsafe { ((*answer).code, CStr::from_ptr((*answer).out).to_str().unwrap().to_string(), CStr::from_ptr((*answer).err).to_str().unwrap().to_string()) };
    unsafe { tachys_free(answer) };
    (code, serde_json::from_str(out.trim()).unwrap_or(serde_json::Value::Null), err)
}

#[test]
fn a_command_called_from_c_answers_as_the_program_does() {
    let (code, j, _) = call(&["recipes"], &[]);
    assert_eq!((code, &j["ok"]), (0, &true.into()));
    let st = dir("setup");
    let (code, j, _) = call(&["setup", "--state", &st.0, "--network", "testnet"], &[]);
    assert_eq!((code, j["network"].as_str(), j["address"].as_str().map(|a| &a[..1])), (0, Some("testnet"), Some("3")));
    // without --json the sentences are `out`, as they are the program's stdout
    let a = tachys::call(&["tachys".into(), "version".into()], &[]);
    let own = [CString::new("tachys").unwrap(), CString::new("version").unwrap()];
    let ptrs: Vec<*const c_char> = own.iter().map(|s| s.as_ptr()).collect();
    let answer = unsafe { tachys_call(ptrs.as_ptr(), 2, std::ptr::null(), 0) };
    assert_eq!(unsafe { CStr::from_ptr((*answer).out) }.to_str().unwrap(), a.out);
    unsafe { tachys_free(answer) };
}

#[test]
fn a_password_and_a_phrase_cross_as_input_lines() {
    let st = dir("sealed");
    let (code, j, _) = call(&["wallet", "new", "--state", &st.0, "--pass-stdin", "--json"], &["correct horse"]);
    assert_eq!((code, &j["sealed"]), (0, &true.into()));
    let (code, j, _) = call(&["wallet", "mnemonic", "--state", &st.0, "--pass-stdin", "--json"], &["wrong"]);
    assert_eq!((code, &j["bad_password"], j.get("mnemonic")), (1, &true.into(), None));
    let (code, j, _) = call(&["wallet", "mnemonic", "--state", &st.0, "--pass-stdin", "--json"], &["correct horse"]);
    let phrase = j["mnemonic"].as_str().unwrap().to_string();
    assert_eq!((code, phrase.split(' ').count()), (0, 12));
    // the password first, then the phrase, as two lines; a phrase in Japanese is three bytes a character
    let other = dir("restored");
    let (_, r, _) = call(&["wallet", "restore", "--state", &other.0, "--mnemonic-file", "-", "--embed", "--pass-stdin", "--json"], &["pässwörd 合言葉", &phrase]);
    assert_eq!(&r["restored"], &serde_json::Value::from("new"), "written, though with no node named its boxes are not looked for yet: {r}");
    let (code, j, _) = call(&["wallet", "mnemonic", "--state", &other.0, "--pass-stdin", "--json"], &["pässwörd 合言葉"]);
    assert_eq!((code, j["mnemonic"].as_str()), (0, Some(phrase.as_str())));
}

#[test]
fn what_is_not_a_call_is_answered_as_misuse_and_the_process_goes_on() {
    // no arguments at all, a list that is not there, an argument that is NULL, one that is not UTF-8
    let nothing = unsafe { tachys_call(std::ptr::null(), 0, std::ptr::null(), 0) };
    assert_eq!(unsafe { (*nothing).code }, 2, "the program with no command is misuse");
    unsafe { tachys_free(nothing) };
    let missing = unsafe { tachys_call(std::ptr::null(), 3, std::ptr::null(), 0) };
    let hole = [c"tachys".as_ptr(), std::ptr::null()];
    let holed = unsafe { tachys_call(hole.as_ptr(), 2, std::ptr::null(), 0) };
    let bad = [c"tachys".as_ptr(), c"qr".as_ptr(), c"\xff\xfe secret".as_ptr()];
    let not_utf8 = unsafe { tachys_call(bad.as_ptr(), 3, std::ptr::null(), 0) };
    for a in [missing, holed, not_utf8] {
        let (code, out, err) = unsafe { ((*a).code, CStr::from_ptr((*a).out).to_str().unwrap().to_string(), CStr::from_ptr((*a).err).to_str().unwrap().to_string()) };
        let j: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!((code, &j["ok"], &j["exit"]), (2, &false.into(), &2.into()), "{out}");
        assert!(!j["error"].as_str().unwrap().is_empty() && !out.contains("secret") && !err.contains("secret"), "says which, never what it held: {out}");
        unsafe { tachys_free(a) };
    }
    unsafe { tachys_free(std::ptr::null_mut()) };
    let (code, j, _) = call(&["wallet", "bogus", "--json"], &[]);
    assert_eq!((code, &j["ok"]), (2, &false.into()));
    let (code, _, _) = call(&["recipes"], &[]);
    assert_eq!(code, 0, "and the call after a refusal is as good as the first");
}

#[test]
fn calls_from_c_run_at_once_on_threads_with_small_stacks() {
    // a thread an app calls from may have far less stack than the program's main thread: the call brings its own
    let handles: Vec<_> = (0..4).map(|i| std::thread::Builder::new().stack_size(64 << 10).spawn(move || {
        let st = dir(&format!("small{i}"));
        let net = if i % 2 == 0 { "mainnet" } else { "testnet" };
        let (code, j, _) = call(&["setup", "--state", &st.0, "--network", net], &[]);
        (code, net, j["network"].as_str().unwrap_or("").to_string())
    }).unwrap()).collect();
    for h in handles { let (code, asked, got) = h.join().unwrap(); assert_eq!((code, got.as_str()), (0, asked)); }
}
