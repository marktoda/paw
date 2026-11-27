//! What lets libgit2 fetch and push over paw's own connections: SSH logged
//! in with the card (`sync::transport`) and HTTPS with the GitHub sign-in
//! (`github::transport`). libgit2 is built with neither transport of its
//! own; each connection hands it one stream per request instead.
//!
//! Whatever the connection, what the host sends is counted as it arrives
//! against one cap ([`DownloadCap`]), so a host sending far more than a
//! password store is stopped before the pack is on disk.

use std::cell::RefCell;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use git2::transport::{Service, SmartSubtransport, SmartSubtransportStream, Transport};

use crate::PawError;

/// The most one libgit2 call (a clone, a fetch or a push) may receive, by
/// default: 256 MiB. A password store with years of history is a few MiB;
/// a host sending more is broken or hostile.
pub(crate) const DOWNLOAD_LIMIT: u64 = 256 << 20;

/// The URL libgit2 is given. It only selects paw's transport; the
/// connection itself comes from [`ACTIVE`].
const URL: &str = "paw://connection";

/// One of paw's connections to a git host, as libgit2 uses it.
pub(crate) trait Carrier: Send + Sync + 'static {
    /// Whether each request is a stream of its own (smart HTTP), rather than
    /// one stream per service on a connection that stays open (SSH).
    fn stateless(&self) -> bool;

    /// The stream for `service`.
    fn open(self: Arc<Self>, service: Service) -> Result<Box<dyn SmartSubtransportStream>, git2::Error>;

    fn cap(&self) -> &DownloadCap;
}

/// How much one libgit2 call has received, against how much it may.
pub(crate) struct DownloadCap {
    limit: AtomicU64,
    received: AtomicU64,
}

impl DownloadCap {
    pub(crate) fn new(limit: u64) -> Self {
        Self { limit: AtomicU64::new(limit), received: AtomicU64::new(0) }
    }

    #[cfg(feature = "sim")]
    pub(crate) fn set_limit(&self, bytes: u64) {
        self.limit.store(bytes, Ordering::SeqCst);
    }

    pub(crate) fn limit(&self) -> u64 {
        self.limit.load(Ordering::SeqCst)
    }

    /// Counts `bytes` more received; an error once past the limit, which
    /// ends the libgit2 call.
    pub(crate) fn count(&self, bytes: usize) -> io::Result<()> {
        let total = self.received.fetch_add(bytes as u64, Ordering::SeqCst) + bytes as u64;
        if total > self.limit() {
            return Err(io::Error::other("the git host sent too much"));
        }
        Ok(())
    }

    /// Why the call stopped, if it was for receiving too much.
    fn exceeded(&self) -> Option<PawError> {
        let limit = self.limit();
        (self.received.load(Ordering::SeqCst) > limit).then(|| {
            PawError::Git(format!(
                "the git host sent more than {} MiB, far more than a password store; paw stopped the download",
                limit >> 20
            ))
        })
    }
}

thread_local! {
    /// The connection the libgit2 call running on this thread uses.
    /// libgit2 picks a transport by URL scheme through a process-wide
    /// table, which has no room for a per-call value; the call is
    /// synchronous, so the calling thread carries it instead.
    static ACTIVE: RefCell<Option<Arc<dyn Carrier>>> = const { RefCell::new(None) };
}

/// Registers the transport once per process. If libgit2 refused it, every
/// call says so instead of failing as "unsupported URL".
fn register() -> Result<(), PawError> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    let registered = REGISTERED.get_or_init(|| {
        let factory = |remote: &git2::Remote<'_>| {
            let carrier = ACTIVE
                .with(|active| active.borrow().clone())
                .ok_or_else(|| git2::Error::from_str("no connection is open for this call"))?;
            Transport::smart(remote, carrier.stateless(), Subtransport(carrier))
        };
        // SAFETY: libgit2 requires that no other thread creates a transport
        // while the table changes. `OnceLock` serialises registration, and
        // every transport of this scheme is created by `with_remote`, which
        // registers first.
        unsafe { git2::transport::register("paw", factory) }.map_err(|e| e.message().to_owned())
    });
    registered.clone().map_err(|e| PawError::Git(format!("could not set up git's transport: {e}")))
}

struct Subtransport(Arc<dyn Carrier>);

impl SmartSubtransport for Subtransport {
    fn action(&self, _url: &str, service: Service) -> Result<Box<dyn SmartSubtransportStream>, git2::Error> {
        self.0.clone().open(service)
    }

    fn close(&self) -> Result<(), git2::Error> {
        // A stream ends when it is dropped.
        Ok(())
    }
}

/// Runs `operation` against a libgit2 remote whose transport is `carrier`,
/// with its cap counted from zero. A failure is the cap's if it was passed,
/// else what `explain` makes of libgit2's error.
pub(crate) fn with_remote<C: Carrier, T, E: From<PawError>>(
    repo: &git2::Repository,
    carrier: Arc<C>,
    operation: impl FnOnce(&mut git2::Remote<'_>) -> Result<T, git2::Error>,
    explain: impl FnOnce(git2::Error) -> E,
) -> Result<T, E> {
    register()?;
    carrier.cap().received.store(0, Ordering::SeqCst);
    ACTIVE.with(|active| *active.borrow_mut() = Some(carrier.clone()));
    let result = repo.remote_anonymous(URL).and_then(|mut remote| operation(&mut remote));
    ACTIVE.with(|active| *active.borrow_mut() = None);
    result.map_err(|e| match carrier.cap().exceeded() {
        Some(too_big) => too_big.into(),
        None => explain(e),
    })
}
