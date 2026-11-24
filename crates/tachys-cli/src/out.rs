//! What a command says, twice over, and how it ends: sentences for a person, and with `--json` one object for a program.
//!
//! Without `--json` nothing changes: sentences on stdout, complaints on stderr, an exit code.
//! With `--json`, stdout carries exactly one JSON object, written when the command ends, and every sentence goes to
//! stderr instead (kept for people and for the run logs; a program need not read them). The object always has
//! `ok` (the command ended with exit code 0), `exit`, `command` and `v`; when `ok` is false, `error` is the reason
//! in words. What else it holds is the command's own (`docs/json.md`). (The agent commands write an object of their
//! own, with `v` and `ok` but without `exit` or `command`.) A caller decides on `ok` and the fields,
//! never on the wording of a sentence: sentences may quote what a node said, fields never do unless named so.

/// The version of the JSON answers. It changes when a field is renamed, retyped or removed; adding one does not change it.
pub(crate) const V: u64 = 1;

/// Everything a call owns while it runs: whether it answers in JSON, the answer so far, the last things said, and
/// (when called as a library) what it wrote and the lines it may read. Nothing of one call is left for the next.
#[derive(Default)]
pub(crate) struct Call {
    json: bool,
    own_answer: bool,
    report: serde_json::Map<String, serde_json::Value>,
    /// the last complaint, and the last sentence said: the reason given when a command fails
    complaint: Option<String>,
    sentence: Option<String>,
    /// kept here instead of written to the process's stdout and stderr
    capture: Option<(String, String)>,
    /// lines to read in place of the process's stdin (a password, a seed phrase)
    input: Option<std::collections::VecDeque<String>>,
}
/// An answer may be a seed phrase (`wallet mnemonic`) and a line read may be a password: what a call held of either is
/// wiped when the call is done with, not left in freed memory. (What is handed back to the caller is the caller's to wipe.)
impl Drop for Call {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        fn wipe(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::String(s) => s.zeroize(),
                serde_json::Value::Array(a) => a.iter_mut().for_each(wipe),
                serde_json::Value::Object(o) => o.iter_mut().for_each(|(_, v)| wipe(v)),
                _ => {}
            }
        }
        self.report.iter_mut().for_each(|(_, v)| wipe(v));
        if let Some(s) = self.complaint.as_mut() { s.zeroize() }
        if let Some(s) = self.sentence.as_mut() { s.zeroize() }
        if let Some((out, err)) = self.capture.as_mut() { out.zeroize(); err.zeroize() }
        if let Some(q) = self.input.as_mut() { q.iter_mut().for_each(|l| l.zeroize()) }
    }
}
/// The calls under way (`Own::calls`): the last is the current one. A command that runs another (`balance` runs
/// `tips` and `wallet status`) puts a call on top for it and takes it off when it ends.
fn now<R>(f: impl FnOnce(&mut Call) -> R) -> R {
    let own = crate::own::own();
    let mut g = own.calls.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_empty() { g.push(Call::default()); }
    f(g.last_mut().expect("just made sure"))
}
/// The command as its own words: `wallet send`, `tips`. Never what came after them (a file name, a text for a QR code).
fn command_of(args: &[String]) -> String { args.iter().skip(1).take_while(|a| !a.starts_with("--")).take(if args.get(1).is_some_and(|c| c == "wallet") { 2 } else { 1 }).map(|s| s.as_str()).collect::<Vec<_>>().join(" ") }

/// Called first: a new call, of which command, answering how. `capture`: keep what it writes instead of printing it.
/// `input`: lines it reads instead of stdin.
pub(crate) fn start(args: &[String], capture: bool, input: Option<Vec<String>>) {
    // (field by field: a type that wipes itself when dropped cannot be made from another's parts)
    let mut c = Call::default();
    c.json = args.iter().any(|a| a == "--json");
    c.capture = capture.then(Default::default);
    c.input = input.map(Into::into);
    c.report.insert("command".into(), command_of(args).into());
    let own = crate::own::own();
    let mut g = own.calls.lock().unwrap_or_else(|p| p.into_inner());
    g.clear();
    g.push(c);
}
/// What a captured call wrote: (stdout, stderr).
pub(crate) fn written() -> (String, String) { now(|c| c.capture.clone().unwrap_or_default()) }
/// The call is over: the lines it was handed and did not read are wiped (a password, a seed phrase).
pub(crate) fn forget_input() { now(|c| if let Some(q) = c.input.as_mut() { q.iter_mut().for_each(zeroize::Zeroize::zeroize); q.clear(); }) }
/// A call of another command from inside this one: its own answer, in JSON, its sentences kept.
pub(crate) fn begin_inner(args: &[String]) {
    let mut c = Call::default();
    c.json = true;
    c.capture = Some(Default::default());
    c.report.insert("command".into(), command_of(args).into());
    crate::own::own().calls.lock().unwrap_or_else(|p| p.into_inner()).push(c);
}
/// The inner call is over: its answer and what it said.
pub(crate) fn end_inner(code: i32) -> (serde_json::Value, String) {
    let own = now(|c| c.own_answer);
    let a = answer(code);
    let mut c = crate::own::own().calls.lock().unwrap_or_else(|p| p.into_inner()).pop().unwrap_or_default();
    let (out, err) = c.capture.take().unwrap_or_default();
    (if own { serde_json::from_str(out.trim()).unwrap_or(serde_json::Value::Null) } else { a }, err)
}
pub(crate) fn json() -> bool { now(|c| c.json) }
/// A line for stdout, or for the call's own record if it is captured.
pub(crate) fn write_out(s: &str) { if !now(|c| match &mut c.capture { Some(k) => { k.0.push_str(s); k.0.push('\n'); true } None => false }) { println!("{s}") } }
fn write_err(s: &str) { if !now(|c| match &mut c.capture { Some(k) => { k.1.push_str(s); k.1.push('\n'); true } None => false }) { eprintln!("{s}") } }
/// The next line of input: one the caller handed over, or a line of the process's stdin.
pub(crate) fn read_line() -> String {
    // the line may be a password or a seed phrase: the copy it was handed over in is wiped once it has been read
    if let Some(mut l) = now(|c| c.input.as_mut().map(|q| q.pop_front().unwrap_or_default())) { let read = l.trim().to_string(); zeroize::Zeroize::zeroize(&mut l); return read }
    let mut s = String::new(); let _ = std::io::stdin().read_line(&mut s); s.trim().to_string()
}

/// A sentence for a person (`say!`).
pub(crate) fn said(s: &str) {
    if let Some(l) = s.lines().rev().find(|l| !l.trim().is_empty()) { now(|c| c.sentence = Some(l.trim().to_string())); }
    if json() { write_err(s) } else { write_out(s) }
}
/// A complaint: why the command cannot go on (`complain!`). It becomes `error` if the command then fails.
pub(crate) fn complained(s: &str) {
    now(|c| c.complaint = Some(s.trim().to_string()));
    write_err(s);
}

/// A field of the answer.
pub(crate) fn set(key: &str, value: impl serde::Serialize) { let v = serde_json::to_value(value).unwrap_or(serde_json::Value::Null); now(|c| { c.report.insert(key.to_string(), v); }) }
/// Takes a field back out (a second pass is about to fill it again).
pub(crate) fn unset(key: &str) { now(|c| { c.report.remove(key); }) }
/// One more entry in a field that is a list.
pub(crate) fn push(key: &str, value: impl serde::Serialize) {
    let v = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    now(|c| { let e = c.report.entry(key.to_string()).or_insert_with(|| serde_json::json!([])); if let Some(a) = e.as_array_mut() { a.push(v); } })
}

/// The answer as it stands, closed with how the command ended.
pub(crate) fn answer(code: i32) -> serde_json::Value {
    now(|c| {
        let mut r = c.report.clone();
        r.insert("v".into(), V.into()); r.insert("ok".into(), (code == 0).into()); r.insert("exit".into(), code.into());
        if code != 0 { r.insert("error".into(), c.complaint.clone().or(c.sentence.clone()).unwrap_or_else(|| "failed".into()).into()); }
        serde_json::Value::Object(r)
    })
}
/// With `--json`, the one object, written once the command has ended with `code`.
pub(crate) fn finish(code: i32) { if json() && !now(|c| c.own_answer) { let a = answer(code).to_string(); write_out(&a); } }
/// The agent commands write an object of their own (`agent.rs`); `finish` then writes nothing.
pub(crate) fn own_answer() { now(|c| c.own_answer = true) }
/// Has this call written its own object yet?
pub(crate) fn answered() -> bool { now(|c| c.own_answer) }

/// A command that stops before its end: the exit code (1: refused or failed; 2: misuse) and why, unless the reason
/// has been said already. No command ends the process itself: this is handed back up to `main`, which is the one
/// place that does, so the commands can be called from a program that goes on living (`tachys::call`).
#[derive(Debug, PartialEq)]
pub(crate) struct Fail { pub(crate) code: i32, pub(crate) msg: Option<String> }
impl Fail {
    pub(crate) fn new(code: i32, msg: String) -> Self { Fail { code, msg: Some(msg) } }
    /// The reason has been said (`say!`) already; only the code is left to hand back.
    pub(crate) fn said(code: i32) -> Self { Fail { code, msg: None } }
    /// The reason, for a test or a caller that wants it as text.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str { self.msg.as_deref().unwrap_or("") }
}
impl From<String> for Fail { fn from(m: String) -> Self { Fail::new(1, m) } }
impl From<&str> for Fail { fn from(m: &str) -> Self { Fail::new(1, m.to_string()) } }
/// What a command returns.
pub(crate) type Ended = Result<(), Fail>;

/// `println!` for a command: a sentence for a person, which `--json` moves to stderr.
#[macro_export]
macro_rules! say { ($($a:tt)*) => { $crate::out::said(&format!($($a)*)) } }
/// `eprintln!` for a command: a complaint, remembered as the reason if the command fails.
#[macro_export]
macro_rules! complain { ($($a:tt)*) => { $crate::out::complained(&format!($($a)*)) } }
