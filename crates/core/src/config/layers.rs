//! The canonical env-layer order (FR-E-02, decision Q-13).
//!
//! The slot def-use analysis ([`super::defuse`]) and the runtime executor
//! ([`crate::run`]) both walk the four env scopes as one linear sequence.
//! A slot written by one layer is only visible to the layers and test
//! commands that come after it, so the two consumers must agree on the
//! order or the analysis describes a sequence that never happens.
//!
//! Keeping the order here — as a single constant — instead of re-listing
//! it inside each consumer makes drift structurally impossible: both sides
//! iterate [`ENTRY_ORDER`], so reordering one requires reordering the
//! other in the same edit.

/// One of the four env scopes (requirement spec 7.3 field table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvScope {
    /// Process-level env: applied once at process start and end (FR-E-04).
    Process,
    /// Global env: the singular `env` table, applied first among the
    /// per-run layers and torn down last (FR-E-02).
    Global,
    /// Case-level env: wraps the tests listed in `envs[].tests`.
    Case,
    /// Thread-level env: applied per worker thread (FR-E-02).
    Thread,
}

/// Entry order of the four env scopes (Q-13): process, global, case,
/// thread.
///
/// Exit phases run in the reverse order: thread, case, global, process.
/// The def-use analysis walks this exact order for the entry prefix and
/// its reverse for the exit suffix; the runtime does the same.
pub const ENTRY_ORDER: [EnvScope; 4] = [
    EnvScope::Process,
    EnvScope::Global,
    EnvScope::Case,
    EnvScope::Thread,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_order_is_process_global_case_thread__F_E_02() {
        // This is the single source of truth that both the def-use
        // analysis and the runtime consume; pinning it here prevents the
        // two from drifting apart again (decision Q-13).
        assert_eq!(
            ENTRY_ORDER,
            [
                EnvScope::Process,
                EnvScope::Global,
                EnvScope::Case,
                EnvScope::Thread,
            ]
        );
    }
}
