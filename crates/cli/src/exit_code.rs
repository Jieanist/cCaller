//! Process exit codes of the ccaller binary.
//!
//! Contract (requirement FR-X-02 / spec section 7.5), relied upon by CI:
//!
//! - `0` all selected cases passed (skipped cases are allowed)
//! - `1` at least one case failed (assertion, env, unresolved reference)
//! - `2` configuration or environment error detected at load time;
//!   CLI usage errors share this code as user-input mistakes
//! - `3` framework internal error (a bug in cCaller itself)
//!
//! A run that executes zero cases must exit nonzero unless the user
//! explicitly passes `--allow-empty` (decision Q-08); that behavior
//! lands with the `run` subcommand in milestone M2/M3.

/// Exit codes of the ccaller binary; see the module docs for the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
// Success and ConfigError are exercised by the check subcommand (M1);
// TestFailed and InternalError land with the run subcommand (M2/M3).
#[allow(dead_code)]
pub enum ExitCode {
    /// All selected cases passed; skipped cases are allowed.
    Success = 0,
    /// At least one case failed.
    TestFailed = 1,
    /// Configuration or environment error detected at load time.
    ConfigError = 2,
    /// Framework internal error (a bug in cCaller itself).
    InternalError = 3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_the_documented_contract__F_G_03() {
        // Pin the numeric contract: CI pipelines branch on these values.
        assert_eq!(ExitCode::Success as i32, 0);
        assert_eq!(ExitCode::TestFailed as i32, 1);
        assert_eq!(ExitCode::ConfigError as i32, 2);
        assert_eq!(ExitCode::InternalError as i32, 3);
    }
}
