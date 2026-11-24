//! What a call owns while it runs, so that calls can run at the same time in one process and none leaves anything
//! for the next: the answer under way, the network, the peer protocol's magic and declaration, and the connections
//! kept open to nodes. Every thread of a call is told which call it works
//! for (`enter` on the thread that makes the call, `join` on each thread of the runtime it starts), and reads and
//! writes that call's own. A thread no call has taken shares one (`SHARED`): a test that runs a command's parts.

use std::cell::RefCell;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, RwLock};

pub(crate) struct Own {
    /// the answer under way, and those of the commands it runs inside itself (`out.rs`)
    pub(crate) calls: Mutex<Vec<crate::out::Call>>,
    pub(crate) net: RwLock<&'static crate::network::Net>,
    wire: tachys_net::Own,
    /// the HTTP clients, with the connections they keep open: for requests to a node (`util::http`), and the quicker
    /// one that finds nodes (`nodes.rs`)
    pub(crate) http: OnceLock<ureq::Agent>,
    pub(crate) http_quick: OnceLock<ureq::Agent>,
}
impl Own {
    pub(crate) fn new() -> Arc<Own> { Arc::new(Own { calls: Mutex::new(Vec::new()), net: RwLock::new(&crate::network::MAINNET), wire: tachys_net::Own::default(), http: OnceLock::new(), http_quick: OnceLock::new() }) }
}
static SHARED: LazyLock<Arc<Own>> = LazyLock::new(Own::new);
thread_local! { static OWN: RefCell<Option<Arc<Own>>> = const { RefCell::new(None) }; }

/// The call this thread works for.
pub(crate) fn own() -> Arc<Own> { OWN.with(|o| o.borrow().clone()).unwrap_or_else(|| SHARED.clone()) }

/// This thread works for `own` from here on: a thread of the call's runtime, which ends with the call.
pub(crate) fn join(own: &Arc<Own>) { own.wire.enter(); OWN.with(|o| *o.borrow_mut() = Some(own.clone())); }

/// The calling thread works for `own` until this is dropped, and then for what it worked for before.
pub(crate) struct Entered(Option<Arc<Own>>, Option<tachys_net::Own>);
pub(crate) fn enter(own: &Arc<Own>) -> Entered { let wire = own.wire.enter(); Entered(OWN.with(|o| o.replace(Some(own.clone()))), wire) }
impl Drop for Entered { fn drop(&mut self) { OWN.with(|o| *o.borrow_mut() = self.0.take()); tachys_net::Own::leave(self.1.take()); } }

/// For a test that runs a command's parts on a runtime it makes: one whose threads work for a call of their own, as
/// the threads of `crate::call`'s do, so that two such tests side by side do not write one answer.
#[cfg(test)]
pub(crate) fn runtime_of_its_own() -> tokio::runtime::Runtime {
    let own = Own::new();
    tokio::runtime::Builder::new_multi_thread().enable_all().on_thread_start(move || join(&own)).build().unwrap()
}
