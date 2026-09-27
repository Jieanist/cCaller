//! The run report and the [`Reporter`] extension point (FR-R-01/02).
//!
//! The executor produces a [`RunReport`] - a summary plus the failing
//! cases - and hands it to a [`Reporter`] to render. The console
//! implementation writes to any [`std::io::Write`], so the CLI decides
//! that results go to stdout while logs go to stderr. No ANSI colour is
//! emitted: non-TTY output must stay clean (FR-R-01).

use std::io::{self, Write};

/// Counters of one run's outcome (requirement spec 7.5 summary).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunSummary {
    /// Number of sub-cases executed.
    pub total: usize,
    /// Number of sub-cases that finished without any failure.
    pub success: usize,
    /// Number of sub-cases with at least one failure (env or assertion).
    pub failure: usize,
    /// Number of Cmds skipped by `CCALLER_ERR_SKIP` (decision Q-01).
    pub skipped: usize,
}

/// One failed sub-case listed in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCase {
    /// Display name of the sub-case (Q-05 format).
    pub name: String,
    /// Human-readable reason, e.g. `expect_eq 5 (actual 0)`.
    pub reason: String,
}

/// The complete outcome of one run, ready for a [`Reporter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// The aggregate counters.
    pub summary: RunSummary,
    /// Every failed sub-case, in execution order.
    pub failures: Vec<FailedCase>,
}

/// Renders a [`RunReport`] to a sink.
///
/// Extension point (FR-R-06): a JSON or JUnit reporter implements this
/// trait next to the console one without touching the executor.
pub trait Reporter {
    /// Renders `report` to this reporter's sink.
    ///
    /// # Errors
    /// Returns the sink's I/O error when writing fails (e.g. a closed
    /// stdout pipe).
    fn report(&mut self, report: &RunReport) -> io::Result<()>;
}

/// The console reporter: one summary line plus a failing-case list.
///
/// Output is plain text with no colour, so pipes and CI logs stay clean
/// (FR-R-01). The sink is injected, keeping this implementation free of
/// any direct stdout/stderr dependency.
pub struct ConsoleReporter<W: Write> {
    out: W,
}

impl<W: Write> ConsoleReporter<W> {
    /// Builds a console reporter writing to `out`.
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> Reporter for ConsoleReporter<W> {
    fn report(&mut self, report: &RunReport) -> io::Result<()> {
        writeln!(
            self.out,
            "Total: {}  Success: {}  Failure: {}  skipped: {}",
            report.summary.total,
            report.summary.success,
            report.summary.failure,
            report.summary.skipped
        )?;
        if !report.failures.is_empty() {
            writeln!(self.out)?;
            writeln!(self.out, "Failed cases:")?;
            for failed in &report.failures {
                writeln!(self.out, "  {}: {}", failed.name, failed.reason)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_line_lists_all_four_counters__F_R_01() {
        let report = RunReport {
            summary: RunSummary {
                total: 3,
                success: 2,
                failure: 1,
                skipped: 4,
            },
            failures: vec![],
        };
        let mut buf = Vec::new();
        ConsoleReporter::new(&mut buf).report(&report).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("Total: 3"), "{text}");
        assert!(text.contains("Success: 2"), "{text}");
        assert!(text.contains("Failure: 1"), "{text}");
        assert!(text.contains("skipped: 4"), "{text}");
        // No ANSI escape may leak into non-TTY output (FR-R-01).
        assert!(!text.contains('\u{1b}'), "{text:?}");
    }

    #[test]
    fn failing_cases_are_listed_with_reasons__F_R_01() {
        let report = RunReport {
            summary: RunSummary {
                total: 1,
                success: 0,
                failure: 1,
                skipped: 0,
            },
            failures: vec![FailedCase {
                name: "t".to_string(),
                reason: "expect_eq 5 (actual 0)".to_string(),
            }],
        };
        let mut buf = Vec::new();
        ConsoleReporter::new(&mut buf).report(&report).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("Failed cases:"), "{text}");
        assert!(text.contains("t: expect_eq 5 (actual 0)"), "{text}");
    }

    #[test]
    fn empty_failures_produce_no_list_header__F_R_01() {
        let report = RunReport {
            summary: RunSummary {
                total: 1,
                success: 1,
                failure: 0,
                skipped: 0,
            },
            failures: vec![],
        };
        let mut buf = Vec::new();
        ConsoleReporter::new(&mut buf).report(&report).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.contains("Failed cases:"), "{text}");
    }
}
