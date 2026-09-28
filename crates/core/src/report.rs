//! The run report and the [`Reporter`] extension points (FR-R-01/02).
//!
//! The executor produces a [`RunReport`] - a summary, one outcome per
//! executed case, and the perf samples of the commands that asked to be
//! timed (FR-P-01) - and hands it to a [`Reporter`] to render. The
//! console implementation writes to any [`std::io::Write`], so the CLI
//! decides that results go to stdout while logs go to stderr. The JSON
//! implementation renders the machine-readable run contract; the JUnit
//! implementation renders the de-facto standard XML schema that CI
//! systems ingest. No ANSI colour is emitted: non-TTY output must stay
//! clean (FR-R-01).

use std::io::{self, Write};
use std::time::Duration;

use serde_json::json;

/// JSON contract version of the run report.
///
/// Mirrors the versioning style of the `check` 7.6 contract: the shape
/// below evolves together with this number.
pub const RUN_REPORT_SCHEMA: u64 = 1;

/// Counters of one run's outcome (requirement spec 7.5 summary).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunSummary {
    /// Number of executed cases (one sub-case per worker replica).
    pub total: usize,
    /// Number of cases that finished without any failure.
    pub success: usize,
    /// Number of cases with at least one failure (env or assertion).
    pub failure: usize,
    /// Number of skips: Cmds that returned `CCALLER_ERR_SKIP` (decision
    /// Q-01) plus cases skipped because isolation is unavailable
    /// (FR-T-05).
    pub skipped: usize,
}

/// Outcome of one executed case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseStatus {
    /// Every Cmd passed (skipped Cmds do not fail a case).
    Passed,
    /// At least one Cmd or env phase failed.
    Failed,
    /// The case never ran (death test without isolation, FR-T-05).
    Skipped,
}

impl CaseStatus {
    /// The machine-readable name used by the JSON contract.
    pub fn as_str(&self) -> &'static str {
        match self {
            CaseStatus::Passed => "passed",
            CaseStatus::Failed => "failed",
            CaseStatus::Skipped => "skipped",
        }
    }
}

/// One executed case listed in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseOutcome {
    /// Display name of the case: the sub-case name (Q-05), prefixed by
    /// its concurrency group and suffixed by its replica index when
    /// more than one worker ran it.
    pub name: String,
    /// Whether the case passed, failed, or was skipped.
    pub status: CaseStatus,
    /// Human-readable reasons, `None` unless the case failed; the same
    /// text the console reporter lists.
    pub reason: Option<String>,
}

/// One timed command invocation (FR-P-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfSample {
    /// Where the timed command ran: a case display name plus command
    /// index for test commands, or an env scope label for env commands.
    pub context: String,
    /// Name of the invoked function.
    pub opfunc: String,
    /// Wall-clock duration of the invocation itself.
    pub duration: Duration,
}

/// The complete outcome of one run, ready for a [`Reporter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// The aggregate counters.
    pub summary: RunSummary,
    /// Every executed case, in execution order.
    pub cases: Vec<CaseOutcome>,
    /// Perf samples of the commands that declared `perf = true`.
    pub perf: Vec<PerfSample>,
}

impl RunReport {
    /// The failed cases, in execution order.
    pub fn failures(&self) -> Vec<&CaseOutcome> {
        self.cases
            .iter()
            .filter(|case| case.status == CaseStatus::Failed)
            .collect()
    }

    /// Renders the machine-readable JSON of the run report.
    ///
    /// Built through [`serde_json::Value`] like the check contract:
    /// rendering cannot fail, and object key order is not part of the
    /// contract. The `reason` key is present only for failed cases.
    pub fn to_json(&self) -> String {
        let cases: Vec<_> = self
            .cases
            .iter()
            .map(|case| {
                let mut value = json!({
                    "name": case.name,
                    "status": case.status.as_str(),
                });
                if let Some(reason) = &case.reason {
                    value["reason"] = json!(reason);
                }
                value
            })
            .collect();
        let perf: Vec<_> = self
            .perf
            .iter()
            .map(|sample| {
                json!({
                    "context": sample.context,
                    "opfunc": sample.opfunc,
                    "duration_ns": duration_ns(sample.duration),
                })
            })
            .collect();
        json!({
            "schema": RUN_REPORT_SCHEMA,
            "ok": self.summary.failure == 0,
            "summary": {
                "total": self.summary.total,
                "success": self.summary.success,
                "failure": self.summary.failure,
                "skipped": self.summary.skipped,
            },
            "cases": cases,
            "perf": perf,
        })
        .to_string()
    }
}

/// Nanoseconds of `duration` as a `u64`; saturates instead of panicking.
fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// Renders a [`RunReport`] to a sink.
///
/// Extension point (FR-R-06): the console, JSON, and JUnit reporters
/// implement this trait next to the executor without touching it.
pub trait Reporter {
    /// Renders `report` to this reporter's sink.
    ///
    /// # Errors
    /// Returns the sink's I/O error when writing fails (e.g. a closed
    /// stdout pipe).
    fn report(&mut self, report: &RunReport) -> io::Result<()>;
}

/// The console reporter: one summary line, the failing-case list, and
/// the perf samples.
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
        let failures = report.failures();
        if !failures.is_empty() {
            writeln!(self.out)?;
            writeln!(self.out, "Failed cases:")?;
            for failed in failures {
                let reason = failed.reason.as_deref().unwrap_or("");
                writeln!(self.out, "  {}: {}", failed.name, reason)?;
            }
        }
        if !report.perf.is_empty() {
            writeln!(self.out)?;
            writeln!(self.out, "Perf samples:")?;
            for sample in &report.perf {
                writeln!(
                    self.out,
                    "  {} {}: {:?}",
                    sample.context, sample.opfunc, sample.duration
                )?;
            }
        }
        Ok(())
    }
}

/// The JSON reporter: the run contract of [`RunReport::to_json`] as one
/// line, machine-consumable the same way `check --format json` is.
pub struct JsonReporter<W: Write> {
    out: W,
}

impl<W: Write> JsonReporter<W> {
    /// Builds a JSON reporter writing to `out`.
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> Reporter for JsonReporter<W> {
    fn report(&mut self, report: &RunReport) -> io::Result<()> {
        writeln!(self.out, "{}", report.to_json())
    }
}

/// The JUnit reporter: the de-facto XML schema CI systems ingest.
///
/// One `testsuite` holds every case; failures carry their reason and
/// skipped cases carry an empty `<skipped/>`. Perf samples are appended
/// as a `<system-out>` block, the schema's escape hatch for extra data.
pub struct JunitReporter<W: Write> {
    out: W,
}

impl<W: Write> JunitReporter<W> {
    /// Builds a JUnit reporter writing to `out`.
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> Reporter for JunitReporter<W> {
    fn report(&mut self, report: &RunReport) -> io::Result<()> {
        let summary = report.summary;
        writeln!(self.out, r#"<?xml version="1.0" encoding="UTF-8"?>"#)?;
        writeln!(
            self.out,
            r#"<testsuites name="ccaller" tests="{}" failures="{}" skipped="{}">"#,
            summary.total, summary.failure, summary.skipped
        )?;
        writeln!(
            self.out,
            r#"  <testsuite name="ccaller" tests="{}" failures="{}" skipped="{}">"#,
            summary.total, summary.failure, summary.skipped
        )?;
        for case in &report.cases {
            match case.status {
                CaseStatus::Passed => {
                    writeln!(
                        self.out,
                        r#"    <testcase name="{}" classname="ccaller"/>"#,
                        xml_escape(&case.name)
                    )?;
                }
                CaseStatus::Failed => {
                    let reason = xml_escape(case.reason.as_deref().unwrap_or(""));
                    writeln!(
                        self.out,
                        r#"    <testcase name="{}" classname="ccaller">"#,
                        xml_escape(&case.name)
                    )?;
                    writeln!(self.out, r#"      <failure message="{}"/>"#, reason)?;
                    writeln!(self.out, "    </testcase>")?;
                }
                CaseStatus::Skipped => {
                    writeln!(
                        self.out,
                        r#"    <testcase name="{}" classname="ccaller">"#,
                        xml_escape(&case.name)
                    )?;
                    writeln!(self.out, "      <skipped/>")?;
                    writeln!(self.out, "    </testcase>")?;
                }
            }
        }
        if !report.perf.is_empty() {
            writeln!(self.out, "    <system-out>")?;
            for sample in &report.perf {
                writeln!(
                    self.out,
                    "      {}: {} {:?}",
                    xml_escape(&sample.context),
                    xml_escape(&sample.opfunc),
                    sample.duration
                )?;
            }
            writeln!(self.out, "    </system-out>")?;
        }
        writeln!(self.out, "  </testsuite>")?;
        writeln!(self.out, "</testsuites>")?;
        Ok(())
    }
}

/// Escapes the five characters XML reserves inside attribute values and
/// text nodes.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-case report: one pass, one fail, plus one perf sample.
    fn sample_report() -> RunReport {
        RunReport {
            summary: RunSummary {
                total: 2,
                success: 1,
                failure: 1,
                skipped: 0,
            },
            cases: vec![
                CaseOutcome {
                    name: "t_pass".to_string(),
                    status: CaseStatus::Passed,
                    reason: None,
                },
                CaseOutcome {
                    name: "t_fail".to_string(),
                    status: CaseStatus::Failed,
                    reason: Some("expect_eq 5 (actual 0)".to_string()),
                },
            ],
            perf: vec![PerfSample {
                context: "t_pass cmd 0".to_string(),
                opfunc: "Call_malloc".to_string(),
                duration: Duration::from_nanos(1234),
            }],
        }
    }

    #[test]
    fn summary_line_lists_all_four_counters__F_R_01() {
        let report = RunReport {
            summary: RunSummary {
                total: 3,
                success: 2,
                failure: 1,
                skipped: 4,
            },
            cases: Vec::new(),
            perf: Vec::new(),
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
        let mut buf = Vec::new();
        ConsoleReporter::new(&mut buf)
            .report(&sample_report())
            .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("Failed cases:"), "{text}");
        assert!(text.contains("t_fail: expect_eq 5 (actual 0)"), "{text}");
        // Passing cases are not listed as failures.
        assert!(!text.contains("t_pass:"), "{text}");
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
            cases: vec![CaseOutcome {
                name: "t".to_string(),
                status: CaseStatus::Passed,
                reason: None,
            }],
            perf: Vec::new(),
        };
        let mut buf = Vec::new();
        ConsoleReporter::new(&mut buf).report(&report).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.contains("Failed cases:"), "{text}");
        assert!(!text.contains("Perf samples:"), "{text}");
    }

    #[test]
    fn perf_samples_are_listed_after_the_summary__F_P_01() {
        let mut buf = Vec::new();
        ConsoleReporter::new(&mut buf)
            .report(&sample_report())
            .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("Perf samples:"), "{text}");
        assert!(text.contains("t_pass cmd 0 Call_malloc"), "{text}");
        assert!(text.contains("1.234µs"), "{text}");
    }

    #[test]
    fn failures_iterator_returns_only_failed_cases__F_R_01() {
        let report = sample_report();
        let failures = report.failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "t_fail");
    }

    #[test]
    fn to_json_renders_the_run_contract__F_R_06() {
        let json: serde_json::Value = serde_json::from_str(&sample_report().to_json()).unwrap();
        assert_eq!(json["schema"].as_u64(), Some(RUN_REPORT_SCHEMA));
        assert_eq!(json["ok"].as_bool(), Some(false));
        assert_eq!(json["summary"]["total"].as_u64(), Some(2));
        assert_eq!(json["summary"]["success"].as_u64(), Some(1));
        assert_eq!(json["summary"]["failure"].as_u64(), Some(1));
        assert_eq!(json["summary"]["skipped"].as_u64(), Some(0));
        assert_eq!(json["cases"][0]["name"].as_str(), Some("t_pass"));
        assert_eq!(json["cases"][0]["status"].as_str(), Some("passed"));
        assert!(json["cases"][0].get("reason").is_none());
        assert_eq!(json["cases"][1]["status"].as_str(), Some("failed"));
        assert_eq!(
            json["cases"][1]["reason"].as_str(),
            Some("expect_eq 5 (actual 0)")
        );
        assert_eq!(json["perf"][0]["opfunc"].as_str(), Some("Call_malloc"));
        assert_eq!(json["perf"][0]["duration_ns"].as_u64(), Some(1234));
        assert_eq!(json["perf"][0]["context"].as_str(), Some("t_pass cmd 0"));
    }

    #[test]
    fn json_reporter_writes_the_contract_to_the_sink__F_R_06() {
        let mut buf = Vec::new();
        JsonReporter::new(&mut buf)
            .report(&sample_report())
            .unwrap();
        let text = String::from_utf8(buf).unwrap();
        let json: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(json["schema"].as_u64(), Some(RUN_REPORT_SCHEMA));
    }

    #[test]
    fn junit_reporter_renders_pass_fail_skip_and_perf__F_R_06() {
        let mut report = sample_report();
        report.cases.push(CaseOutcome {
            name: "t_skip".to_string(),
            status: CaseStatus::Skipped,
            reason: None,
        });
        report.summary = RunSummary {
            total: 3,
            success: 1,
            failure: 1,
            skipped: 1,
        };
        let mut buf = Vec::new();
        JunitReporter::new(&mut buf).report(&report).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.starts_with("<?xml"), "{text}");
        assert!(
            text.contains(r#"testsuites name="ccaller" tests="3" failures="1" skipped="1""#),
            "{text}"
        );
        assert!(
            text.contains(r#"<testcase name="t_pass" classname="ccaller"/>"#),
            "{text}"
        );
        assert!(
            text.contains("<failure message=\"expect_eq 5 (actual 0)\"/>"),
            "{text}"
        );
        assert!(text.contains("<skipped/>"), "{text}");
        assert!(text.contains("<system-out>"), "{text}");
        assert!(text.contains("</testsuites>"), "{text}");
    }

    #[test]
    fn junit_escapes_reserved_characters__F_R_06() {
        let report = RunReport {
            summary: RunSummary {
                total: 1,
                success: 0,
                failure: 1,
                skipped: 0,
            },
            cases: vec![CaseOutcome {
                name: "a<b>&\"c\"".to_string(),
                status: CaseStatus::Failed,
                reason: Some("x & y < z".to_string()),
            }],
            perf: Vec::new(),
        };
        let mut buf = Vec::new();
        JunitReporter::new(&mut buf).report(&report).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains(r#"name="a&lt;b&gt;&amp;&quot;c&quot;""#),
            "{text}"
        );
        assert!(text.contains(r#"message="x &amp; y &lt; z""#), "{text}");
    }

    #[test]
    fn duration_ns_saturates_instead_of_panicking__F_P_01() {
        assert_eq!(duration_ns(Duration::from_nanos(42)), 42);
        let huge = Duration::from_secs(u64::MAX);
        assert_eq!(duration_ns(huge), u64::MAX);
    }
}
