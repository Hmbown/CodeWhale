//! Process-wide test-environment barrier owned by shell dispatch.
//!
//! `shell_dispatcher.rs` is also compiled directly by integration harnesses,
//! outside the main binary crate. Keeping the barrier under that module makes
//! shell detection self-contained while `crate::test_support` re-exports the
//! same instance to its existing environment-mutating callers in the main crate.

use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::thread::ThreadId;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Who currently counts as "inside" the process-wide env lock.
///
/// The owner is the thread holding [`TestEnvLock`]. `adopted` holds helper
/// threads that owner explicitly enrolled with [`join_env_scope`] — see that
/// function for why a worker thread of the current test must not be treated as
/// a foreign reader.
#[derive(Default)]
struct EnvScope {
    /// Bumped on every acquisition, so a ticket minted by an earlier test can
    /// never enroll a thread into a later test's environment.
    generation: u64,
    owner: Option<ThreadId>,
    adopted: Vec<ThreadId>,
}

fn env_scope() -> &'static Mutex<EnvScope> {
    static SCOPE: OnceLock<Mutex<EnvScope>> = OnceLock::new();
    SCOPE.get_or_init(|| Mutex::new(EnvScope::default()))
}

fn lock_env_scope() -> MutexGuard<'static, EnvScope> {
    match env_scope().lock() {
        Ok(scope) => scope,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn open_env_scope() {
    let mut scope = lock_env_scope();
    scope.generation = scope.generation.wrapping_add(1);
    scope.owner = Some(std::thread::current().id());
    scope.adopted.clear();
}

fn current_thread_owns_contended_env_lock() -> bool {
    let scope = lock_env_scope();
    let current = std::thread::current().id();
    scope.owner == Some(current) || scope.adopted.contains(&current)
}

/// Proof that the calling thread owns a live [`lock_test_env`] scope, handed to
/// a worker thread so it can join that scope with [`join_env_scope`].
///
/// Returns `None` when the caller is not the owner, so a ticket can never be
/// minted on behalf of a test that did not seal the environment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EnvScopeTicket {
    generation: u64,
}

impl EnvScopeTicket {
    /// Which sealed environment this ticket authorizes. Callers that gate real
    /// disk writes on a live scope key their bookkeeping by this value, so a
    /// straggler from generation N can never be mistaken for work belonging to
    /// generation N+1.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// The generation of the env scope the calling thread is currently inside, as
/// owner or as a [`join_env_scope`]-adopted worker; `None` when the thread is a
/// foreign reader with no sealed environment of its own.
///
/// This is the authorization primitive for anything that must only touch disk
/// on behalf of a test that actually sealed `HOME`. A process-global "writes
/// are enabled" flag cannot distinguish unrelated parallel tests.
pub(crate) fn current_env_scope_generation() -> Option<u64> {
    let scope = lock_env_scope();
    let current = std::thread::current().id();
    if scope.owner == Some(current) || scope.adopted.contains(&current) {
        Some(scope.generation)
    } else {
        None
    }
}

pub(crate) fn env_scope_ticket() -> Option<EnvScopeTicket> {
    let scope = lock_env_scope();
    (scope.owner == Some(std::thread::current().id())).then_some(EnvScopeTicket {
        generation: scope.generation,
    })
}

/// Enroll the calling thread in the ticket's env scope for as long as the
/// returned guard lives.
///
/// [`with_test_env_lock`] stops a foreign test from resolving another test's
/// temporary `HOME`. A helper thread doing work for the sealing test must see
/// that same environment without blocking on the mutex its owner holds.
pub(crate) fn join_env_scope(ticket: Option<EnvScopeTicket>) -> Option<EnvScopeMembership> {
    let ticket = ticket?;
    let mut scope = lock_env_scope();
    if scope.owner.is_none() || scope.generation != ticket.generation {
        return None;
    }
    let thread = std::thread::current().id();
    if !scope.adopted.contains(&thread) {
        scope.adopted.push(thread);
    }
    Some(EnvScopeMembership {
        generation: ticket.generation,
        thread,
    })
}

pub(crate) struct EnvScopeMembership {
    generation: u64,
    thread: ThreadId,
}

impl Drop for EnvScopeMembership {
    fn drop(&mut self) {
        let mut scope = lock_env_scope();
        if scope.generation == self.generation {
            scope.adopted.retain(|thread| *thread != self.thread);
        }
    }
}

/// Owned process-wide test-environment lock.
///
/// Clearing the owner before the underlying mutex unlocks keeps re-entrant
/// reader detection exact. Closing the scope also evicts adopted workers, so
/// enrollment cannot outlive the test that granted it.
pub(crate) struct TestEnvLock {
    _guard: MutexGuard<'static, ()>,
}

impl Drop for TestEnvLock {
    fn drop(&mut self) {
        let mut scope = lock_env_scope();
        if scope.owner == Some(std::thread::current().id()) {
            scope.owner = None;
            scope.adopted.clear();
        }
    }
}

/// Acquire the process-wide env-var mutex.
///
/// If a prior test panicked while holding the lock, recover the guard instead
/// of cascading failures across unrelated tests.
pub(crate) fn lock_test_env() -> TestEnvLock {
    let guard = match env_lock().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    open_env_scope();
    TestEnvLock { _guard: guard }
}

/// Read process-global test environment while respecting [`lock_test_env`].
///
/// The owner check makes the barrier re-entrant for a test that reads its own
/// guarded override.
pub(crate) fn with_test_env_lock<T>(read: impl FnOnce() -> T) -> T {
    if current_thread_owns_contended_env_lock() {
        return read();
    }

    let _guard = lock_test_env();
    read()
}

pub(crate) fn current_thread_holds_test_env_lock() -> bool {
    match env_lock().try_lock() {
        Ok(guard) => {
            drop(guard);
            false
        }
        Err(TryLockError::Poisoned(poisoned)) => {
            drop(poisoned.into_inner());
            false
        }
        Err(TryLockError::WouldBlock) => current_thread_owns_contended_env_lock(),
    }
}
