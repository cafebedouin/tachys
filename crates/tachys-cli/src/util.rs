//! Small helpers every command uses: arguments, hex, text a node sent, the HTTP client, owner-only files.

use std::time::Duration;

use ergo_chain_types::Header;

/// The value after `name` on the command line, or `default` if `name` is not there. A value that is there but missing
/// or unreadable is an `Err`: it is never quietly replaced by the default (an amount mistyped is not the default amount).
pub(crate) fn try_arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    let Some(i) = args.iter().position(|a| a == name) else { return Ok(default) };
    let Some(v) = args.get(i + 1) else { return Err(format!("{name} needs a value")) };
    v.parse().map_err(|_| format!("{name}: \"{}\" is not a value it takes", clip(v, 60)))
}
/// `try_arg`, as a command reads an option: a value that cannot be read is misuse (exit code 2).
pub(crate) fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, crate::out::Fail> {
    try_arg(args, name, default).map_err(|e| crate::out::Fail::new(2, e))
}
// `OPTIONS`: every option any command reads, gathered from the source when the crate is built (build.rs). A word
// starting with `--` that is not one of these is a slip, and a slip must not be read as "option not given".
include!(concat!(env!("OUT_DIR"), "/options.rs"));
/// Options whose value is free text, which may itself start with `--`.
const FREE_TEXT: &[&str] = &["--name", "--purpose", "--mint"];
/// The first word on the command line that looks like an option and is not one.
pub(crate) fn unknown_option(args: &[String]) -> Option<&str> {
    args.iter().enumerate().skip(1).find(|(i, a)| a.starts_with("--") && !OPTIONS.contains(&a.as_str()) && !FREE_TEXT.contains(&args[i - 1].as_str())).map(|(_, a)| a.as_str())
}
pub(crate) fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
pub(crate) fn id_hex(h: &Header) -> String { let v: Vec<u8> = h.id.0.into(); hex(&v) }
/// The network group a peer counts for: the /16 of an IPv4 address, the /32 of an IPv6 address (an IPv6 address that
/// carries an IPv4 one counts as that IPv4 address). One provider hands out whole IPv6 /64s and /48s, so the full
/// address would let one machine pass for many groups. Anything that is not an address (a host name, a file name
/// from `tachys check`) falls back to its first two dot-separated parts.
pub(crate) fn group_of(addr: &str) -> String {
    use std::net::{IpAddr, SocketAddr};
    let ip = addr.parse::<SocketAddr>().map(|s| s.ip()).or_else(|_| addr.parse::<IpAddr>())
        .or_else(|_| addr.rsplit_once(['_', ':']).map_or(addr, |x| x.0).trim_matches(['[', ']']).parse::<IpAddr>());
    let v4 = |o: [u8; 4]| format!("{}.{}", o[0], o[1]);
    match ip {
        Ok(IpAddr::V4(a)) => v4(a.octets()),
        Ok(IpAddr::V6(a)) => match a.to_ipv4_mapped() { Some(m) => v4(m.octets()), None => format!("{:x}:{:x}::/32", a.segments()[0], a.segments()[1]) },
        Err(_) => addr.split('.').take(2).collect::<Vec<_>>().join("."),
    }
}

/// Hex to bytes. Anything that is not hex digits is refused before it is sliced: a node chooses these strings.
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|c| c.is_ascii_hexdigit()) { return None }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}
/// The first `n` characters of text a node or a file gave us. Never a byte slice: that panics inside a character.
pub(crate) fn clip(s: &str, n: usize) -> String { s.chars().take(n).collect() }
/// What a node said, as one line for a message: no line breaks or control characters, so it cannot pass for a line of ours.
pub(crate) fn one_line(s: &str) -> String { s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(400).collect::<String>().trim().to_string() }

/// The longest one request to a node may take, from connecting to the last byte of its answer. The connect, read and
/// write limits start again with every byte, so without this a node that sends a byte now and then holds a call for as
/// long as it likes. The largest answers read are lists of up to 200 boxes (a box is at most 4 KB, so at most about
/// 2 MB of JSON) and box proofs for up to 100 ids; nothing over 10 MiB is read at all (ureq's `into_string`). Two
/// minutes reads 2 MB at 140 kbit/s.
pub(crate) const WHOLE_REQUEST: Duration = Duration::from_secs(120);

thread_local! { static DEADLINE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) }; }
/// While this is held, every request to a node made on this thread ends by `at` (`tachys wait --timeout`); dropped,
/// the deadline before it is back.
pub(crate) struct Deadline(Option<std::time::Instant>);
impl Deadline {
    pub(crate) fn set(at: std::time::Instant) -> Deadline { Deadline(DEADLINE.with(|d| d.replace(Some(at)))) }
    /// What is left of the deadline set on this thread, if one is.
    pub(crate) fn left() -> Option<Duration> { DEADLINE.with(|d| d.get()).map(|at| at.saturating_duration_since(std::time::Instant::now())) }
}
impl Drop for Deadline { fn drop(&mut self) { DEADLINE.with(|d| d.set(self.0)); } }
/// The time one request may take now: `WHOLE_REQUEST`, or less if a deadline comes sooner (at least a millisecond:
/// a request past its deadline fails at once).
pub(crate) fn request_time() -> Duration { Deadline::left().map_or(WHOLE_REQUEST, |l| l.min(WHOLE_REQUEST)).max(Duration::from_millis(1)) }

/// One HTTP client for every request to a node. A node that takes the connection and then says nothing is given up on,
/// and so is one that answers too slowly to finish within `request_time`.
pub(crate) struct Http(&'static ureq::Agent);
impl Http {
    pub(crate) fn get(&self, url: &str) -> ureq::Request { self.0.get(url).timeout(request_time()) }
    pub(crate) fn post(&self, url: &str) -> ureq::Request { self.0.post(url).timeout(request_time()) }
}
pub(crate) fn http() -> Http {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    Http(AGENT.get_or_init(|| ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(30)).timeout_write(Duration::from_secs(30))
        .timeout(WHOLE_REQUEST)
        // a node is asked at the address it was found or named at, and nowhere it then points: a redirect could send an address lookup or a signed payment to another host
        .redirects(0).build()))
}

/// Write a file that only its owner can read: a wallet file holds a seed phrase, sealed or not.
pub(crate) fn write_private(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
    let mut f = o.open(path)?;
    // a file that was already there keeps the mode it had, so set it as well
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; f.set_permissions(std::fs::Permissions::from_mode(0o600))?; }
    f.write_all(bytes)?;
    f.sync_all()
}
/// Write a file beside its place and move it in, so a crash or a full disk leaves the old file, not half a new one.
pub(crate) fn write_whole(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = format!("{path}.tmp");
    write_private(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path)).inspect_err(|_| { let _ = std::fs::remove_file(&tmp); })
}
/// Make a state directory if it is not there yet, for its owner only. One that exists is left as its owner set it.
pub(crate) fn private_dir(dir: &str) {
    if std::path::Path::new(dir).exists() { return }
    let _ = std::fs::create_dir_all(dir);
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)); }
}

/// "1 token", "3 tokens", "no tokens".
pub(crate) fn count(n: usize, what: &str) -> String { match n { 0 => format!("no {what}s"), 1 => format!("1 {what}"), n => format!("{n} {what}s") } }

/// A line of input: a password or a seed phrase, never an argument or a file. From the caller if this is a library call, else stdin.
pub(crate) fn stdin_line() -> String { crate::out::read_line() }
