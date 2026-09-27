//! Logging bootstrap (requirement FR-R-02).
//!
//! Logs go to stderr (env_logger's default target) so results on
//! stdout and diagnostics can be captured separately by CI. The
//! `-l/--log` option accepts the digits 1-4 or a level name; a set
//! `RUST_LOG` variable takes precedence over the CLI value.

use log::LevelFilter;

/// Parses a `-l/--log` value: `1`..`4` or a level name (case-insensitive).
///
/// # Errors
/// Returns a human-readable message when the value matches neither form.
pub fn parse_log_level(value: &str) -> Result<LevelFilter, String> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "error" => Ok(LevelFilter::Error),
        "2" | "warn" => Ok(LevelFilter::Warn),
        "3" | "info" => Ok(LevelFilter::Info),
        "4" | "debug" => Ok(LevelFilter::Debug),
        _ => Err(format!(
            "invalid log level `{value}`: expected 1-4 or error/warn/info/debug"
        )),
    }
}

/// Initializes the process logger writing to stderr.
///
/// `RUST_LOG` (requirement FR-R-02) wins over `cli_level`; when neither
/// is present the level defaults to `info`.
///
/// # Panics
/// Panics when a logger is already installed; the CLI calls this
/// exactly once at startup, so a second call would be a wiring bug.
pub fn init(cli_level: Option<LevelFilter>) {
    let mut builder = env_logger::Builder::new();
    if std::env::var_os("RUST_LOG").is_some() {
        builder.parse_env(env_logger::Env::default());
    } else {
        builder.filter_level(cli_level.unwrap_or(LevelFilter::Info));
    }
    builder.init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_accepts_digits_and_level_names__F_R_02() {
        assert_eq!(parse_log_level("1"), Ok(LevelFilter::Error));
        assert_eq!(parse_log_level("2"), Ok(LevelFilter::Warn));
        assert_eq!(parse_log_level("3"), Ok(LevelFilter::Info));
        assert_eq!(parse_log_level("4"), Ok(LevelFilter::Debug));
        assert_eq!(parse_log_level("DEBUG"), Ok(LevelFilter::Debug));
        assert_eq!(parse_log_level("warn"), Ok(LevelFilter::Warn));
    }

    #[test]
    fn log_level_rejects_out_of_range_and_garbage__F_R_02() {
        assert!(parse_log_level("0").is_err());
        assert!(parse_log_level("5").is_err());
        assert!(parse_log_level("verbose").is_err());
        assert!(parse_log_level("").is_err());
    }
}
