//! Input group expansion: from declared parameters to concrete sub-cases
//! (FR-C-03/04/05/06).
//!
//! Each InputGroup merges the `shared_inputs` groups named in `refs` with
//! its own `args` (same-name conflicts are errors naming both sources),
//! resolves `$var` references inside its own values against single-valued
//! shared parameters, and expands the cartesian product with parameters
//! visited in sorted name order — the first sorted parameter varies
//! slowest, so the enumeration order is reproducible (FR-C-05).
//!
//! Sub-case names follow the Q-05 format `{test}/{input}#{index}[k=v,...]`
//! with keys sorted by parameter name. Test commands are resolved per
//! sub-case: `$var` in command args and expectations reads the sub-case
//! bindings. Unresolved variables are reported once per
//! (group, command, variable) with the number of affected sub-cases;
//! sub-cases whose commands fail to resolve are dropped, so everything a
//! run plan carries is fully concrete.

use std::collections::{BTreeMap, HashMap, HashSet};

use toml::Spanned;

use crate::error::Location;

use super::cases::{CmdDef, InputGroup, InputValue, TestDef};
use super::diag::{codes, Diagnostic};
use super::source::SourceDoc;
use super::value::{parse_value, ConcreteValue, ScalarRaw, ValueKind};

/// Upper bound on the number of combinations one input group may expand
/// into; larger groups are rejected instead of exhausting memory.
pub const MAX_COMBINATIONS_PER_INPUT_GROUP: usize = 10_000;

/// One expectation of a command after value resolution and negation
/// folding (FR-C-07, FR-V-02).
///
/// The enum of the previous design is gone: the final assertion kind is
/// carried by name and resolved through the assertion registry at
/// evaluation time, so a new assertion needs no change here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExpectation {
    /// The final assertion field name after `!` folding, e.g. `expect_ne`.
    pub kind: &'static str,
    /// The resolved expected value.
    pub value: ConcreteValue,
}

impl ResolvedExpectation {
    /// Evaluates this expectation against the actual return value.
    ///
    /// The registry is authoritative for what the kind means; a kind that
    /// is no longer registered (impossible for a validated configuration)
    /// evaluates to a visible failure instead of panicking.
    pub fn evaluate(&self, actual: i64) -> crate::assertion::AssertionOutcome {
        let passed = crate::assertion::lookup(self.kind)
            .is_some_and(|assertion| assertion.evaluate(&self.value, actual));
        crate::assertion::AssertionOutcome {
            passed,
            expectation: format!("{} {}", self.kind, self.value),
        }
    }
}

/// A command with every value resolved against a sub-case binding.
#[derive(Debug, Clone)]
pub struct ResolvedCmd {
    /// Function name as declared; existence was checked by validation.
    pub opfunc: String,
    /// Parameter name and concrete value, in declared order.
    pub args: Vec<(String, ConcreteValue)>,
    /// The resolved expectation, if the command carries one.
    pub expect: Option<ResolvedExpectation>,
    /// Whether the executor records this call's duration (FR-P-01).
    pub perf: bool,
    /// Per-command timeout in seconds; `0` disables the watchdog (FR-X-03).
    pub timeout: u64,
}

/// One concrete sub-case of a test (FR-C-05).
#[derive(Debug, Clone)]
pub struct SubCase {
    /// Display name, Q-05 format `{test}/{input}#{index}[k=v,...]`;
    /// a test without input groups uses the bare test name.
    pub name: String,
    /// Parameter bindings of this sub-case.
    pub bindings: BTreeMap<String, ConcreteValue>,
    /// Commands with all values resolved.
    pub cmds: Vec<ResolvedCmd>,
    /// Group-level `should_panic` override for this sub-case (FR-T-05);
    /// `None` inherits the test-level flag.
    pub should_panic: Option<bool>,
    /// Group-level `break_if_fail` override for this sub-case (FR-T-01);
    /// `None` inherits the test-level flag.
    pub break_if_fail: Option<bool>,
}

/// Expands every input group of `test` into concrete sub-cases.
///
/// Groups are processed in declaration order and independently: a group
/// whose merging, value resolution, or combination bound fails is skipped
/// after all of its findings are recorded in `out`. A test without input
/// groups yields one sub-case with empty bindings.
pub fn expand_test(
    test: &TestDef,
    shared: &BTreeMap<String, Spanned<BTreeMap<String, Spanned<InputValue>>>>,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) -> Vec<SubCase> {
    let test_name = test.name.get_ref().as_str();
    let mut subcases = Vec::new();
    let mut agg = CmdErrorAggregator::default();

    if test.inputs.is_empty() {
        let bindings = BTreeMap::new();
        if let Some(cmds) = resolve_cmds(test_name, None, &bindings, &test.cmds, doc, &mut agg) {
            subcases.push(SubCase {
                name: test_name.to_string(),
                bindings,
                cmds,
                // No input group: nothing overrides the test-level flags.
                should_panic: None,
                break_if_fail: None,
            });
        }
    }

    for group in &test.inputs {
        let group_ref = group.get_ref();
        let group_name = group_ref.name.get_ref().as_str();
        let Some(params) = merge_group_params(test_name, group_name, group_ref, shared, doc, out)
        else {
            continue;
        };

        let names: Vec<&String> = params.keys().collect();
        let lists: Vec<&Vec<ConcreteValue>> = params.values().collect();
        match combination_count(&lists) {
            Ok(total) => {
                let mut indices = vec![0usize; names.len()];
                for index in 0..total {
                    let bindings: BTreeMap<String, ConcreteValue> = names
                        .iter()
                        .zip(&indices)
                        .zip(&lists)
                        .map(|((name, &i), list)| ((*name).clone(), list[i].clone()))
                        .collect();
                    if let Some(cmds) = resolve_cmds(
                        test_name,
                        Some(group_name),
                        &bindings,
                        &test.cmds,
                        doc,
                        &mut agg,
                    ) {
                        subcases.push(SubCase {
                            name: subcase_name(test_name, group_name, index, &bindings),
                            bindings,
                            cmds,
                            // The group's own flags, when declared, override
                            // the test-level defaults for its sub-cases.
                            should_panic: group_ref.should_panic,
                            break_if_fail: group_ref.break_if_fail,
                        });
                    }
                    advance_odometer(&mut indices, &lists);
                }
            }
            Err(count) => out.push(Diagnostic::new(
                codes::TOO_MANY_COMBINATIONS,
                doc.locate(group_ref.name.span()),
                format!(
                    "test `{test_name}` input `{group_name}` expands to {count} \
                     combinations, above the limit of {MAX_COMBINATIONS_PER_INPUT_GROUP}"
                ),
            )),
        }
    }

    agg.finish(out);
    subcases
}

/// Total combinations of a group, or the offending count when it exceeds
/// the limit.
fn combination_count(lists: &[&Vec<ConcreteValue>]) -> Result<usize, usize> {
    let mut total: usize = 1;
    for list in lists {
        total = match total.checked_mul(list.len()) {
            Some(t) => t,
            None => return Err(usize::MAX),
        };
        if total > MAX_COMBINATIONS_PER_INPUT_GROUP {
            return Err(total);
        }
    }
    Ok(total)
}

/// Advances the odometer: the last (highest-sorted) parameter varies
/// fastest, the first sorted parameter slowest (FR-C-05).
fn advance_odometer(indices: &mut [usize], lists: &[&Vec<ConcreteValue>]) {
    for i in (0..indices.len()).rev() {
        indices[i] += 1;
        if indices[i] < lists[i].len() {
            break;
        }
        indices[i] = 0;
    }
}

/// Builds the Q-05 sub-case name; keys are already sorted by `BTreeMap`.
fn subcase_name(
    test: &str,
    group: &str,
    index: usize,
    bindings: &BTreeMap<String, ConcreteValue>,
) -> String {
    let parts: Vec<String> = bindings.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{test}/{group}#{index}[{}]", parts.join(","))
}

/// Merges `refs` groups and own args into one parameter-to-values map.
///
/// Shared values must be literals; own-args values may reference
/// single-valued shared parameters via `$var` (FR-C-06). Returns `None`
/// once any finding was recorded, so the group is skipped.
fn merge_group_params(
    test: &str,
    group: &str,
    input_group: &InputGroup,
    shared: &BTreeMap<String, Spanned<BTreeMap<String, Spanned<InputValue>>>>,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) -> Option<BTreeMap<String, Vec<ConcreteValue>>> {
    let mut merged: BTreeMap<String, Vec<ConcreteValue>> = BTreeMap::new();
    let mut origin: HashMap<String, String> = HashMap::new();
    let mut ok = true;

    for r in &input_group.refs {
        // Existence is a validation concern; missing keys were reported.
        let Some(shared_group) = shared.get(r.get_ref()) else {
            continue;
        };
        let source = format!("shared_inputs.{}", r.get_ref());
        for (param, value) in shared_group.get_ref() {
            let loc = doc.locate(value.span());
            if origin.contains_key(param) {
                out.push(conflict_diag(param, &origin[param], &source, loc));
                ok = false;
                continue;
            }
            match literal_values(value.get_ref(), &loc, out) {
                Some(values) => {
                    origin.insert(param.clone(), source.clone());
                    merged.insert(param.clone(), values);
                }
                None => ok = false,
            }
        }
    }

    // Own-args `$var` may reference single-valued shared parameters only;
    // referencing own parameters would create order dependencies. The map
    // owns its keys and values so `merged` stays insertable below.
    let shared_singles: BTreeMap<String, ConcreteValue> = merged
        .iter()
        .filter(|(_, values)| values.len() == 1)
        .map(|(name, values)| (name.clone(), values[0].clone()))
        .collect();

    for (param, value) in &input_group.args {
        let loc = doc.locate(value.span());
        if origin.contains_key(param) {
            out.push(conflict_diag(
                param,
                &origin[param],
                "the group's own args",
                loc,
            ));
            ok = false;
            continue;
        }
        let ctx = OwnCtx {
            test,
            group,
            param,
            shared_singles: &shared_singles,
            merged: &merged,
            loc: &loc,
        };
        match resolve_own_values(value.get_ref(), &ctx, out) {
            Some(values) => {
                origin.insert(param.clone(), "the group's own args".to_string());
                merged.insert(param.clone(), values);
            }
            None => ok = false,
        }
    }

    ok.then_some(merged)
}

/// Reports a same-name conflict between two sources (FR-C-06).
fn conflict_diag(param: &str, first: &str, second: &str, loc: Location) -> Diagnostic {
    Diagnostic::new(
        codes::CONFLICTING_INPUT_PARAM,
        loc,
        format!("parameter `{param}` is defined by both {first} and {second}"),
    )
}

/// Materializes a shared parameter into its literal values.
fn literal_values(
    value: &InputValue,
    loc: &Location,
    out: &mut Vec<Diagnostic>,
) -> Option<Vec<ConcreteValue>> {
    match value {
        InputValue::Single(raw) => Some(vec![literal_scalar(raw, loc, out)?]),
        InputValue::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for raw in items {
                values.push(literal_scalar(raw, loc, out)?);
            }
            Some(values)
        }
        InputValue::Range(spec) => {
            let start = literal_bound(&spec.start, loc, out)?;
            let end = literal_bound(&spec.end, loc, out)?;
            let step = literal_bound(&spec.step, loc, out)?;
            if step <= 0 || start > end {
                out.push(Diagnostic::new(
                    codes::INVALID_RANGE,
                    loc.clone(),
                    format!(
                        "range has start {start}, end {end}, step {step} \
                         (need step > 0 and start <= end)"
                    ),
                ));
                return None;
            }
            Some(enumerate_range(start, end, step))
        }
    }
}

/// Resolves one shared scalar; shared values must be literals.
fn literal_scalar(
    raw: &ScalarRaw,
    loc: &Location,
    out: &mut Vec<Diagnostic>,
) -> Option<ConcreteValue> {
    match raw {
        ScalarRaw::Int(i) => Some(ConcreteValue::Int(*i)),
        ScalarRaw::Str(s) => {
            let parsed = match parse_value(s) {
                Ok(p) => p,
                Err(e) => {
                    out.push(Diagnostic::new(
                        codes::INVALID_VALUE,
                        loc.clone(),
                        format!("invalid value `{s}`: {e}"),
                    ));
                    return None;
                }
            };
            if let ValueKind::Var(name) = &parsed.kind {
                out.push(Diagnostic::new(
                    codes::SHARED_NOT_LITERAL,
                    loc.clone(),
                    format!(
                        "shared value `{s}` references `${name}`; \
                         shared values must be literals"
                    ),
                ));
                return None;
            }
            concrete_of(parsed.kind, parsed.negated, loc, out)
        }
    }
}

/// A shared range bound, which must be a literal integer.
fn literal_bound(raw: &ScalarRaw, loc: &Location, out: &mut Vec<Diagnostic>) -> Option<i64> {
    match literal_scalar(raw, loc, out)? {
        ConcreteValue::Int(i) => Some(i),
        ConcreteValue::Str(s) => {
            out.push(Diagnostic::new(
                codes::INVALID_VALUE,
                loc.clone(),
                format!("range bound resolved to string `{s}`, must be an integer"),
            ));
            None
        }
    }
}

/// Everything the own-args resolver needs to know about one parameter.
struct OwnCtx<'a> {
    test: &'a str,
    group: &'a str,
    param: &'a str,
    shared_singles: &'a BTreeMap<String, ConcreteValue>,
    merged: &'a BTreeMap<String, Vec<ConcreteValue>>,
    loc: &'a Location,
}

/// Resolves one own-args parameter into its concrete values, allowing
/// `$var` references to single-valued shared parameters (FR-C-06).
fn resolve_own_values(
    value: &InputValue,
    ctx: &OwnCtx<'_>,
    out: &mut Vec<Diagnostic>,
) -> Option<Vec<ConcreteValue>> {
    let OwnCtx {
        test, group, param, ..
    } = *ctx;
    match value {
        InputValue::Single(raw) => Some(vec![resolve_own_scalar(raw, ctx, out)?]),
        InputValue::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for raw in items {
                values.push(resolve_own_scalar(raw, ctx, out)?);
            }
            Some(values)
        }
        InputValue::Range(spec) => {
            let start = own_bound(&spec.start, ctx, out)?;
            let end = own_bound(&spec.end, ctx, out)?;
            let step = own_bound(&spec.step, ctx, out)?;
            if step <= 0 || start > end {
                out.push(Diagnostic::new(
                    codes::INVALID_RANGE,
                    ctx.loc.clone(),
                    format!(
                        "test `{test}` input `{group}`: range `{param}` \
                         has start {start}, end {end}, step {step} \
                         (need step > 0 and start <= end)"
                    ),
                ));
                return None;
            }
            Some(enumerate_range(start, end, step))
        }
    }
}

/// Resolves one own-args scalar; `$var` looks up single-valued shared
/// parameters, everything else must be a literal.
fn resolve_own_scalar(
    raw: &ScalarRaw,
    ctx: &OwnCtx<'_>,
    out: &mut Vec<Diagnostic>,
) -> Option<ConcreteValue> {
    let OwnCtx {
        test, group, param, ..
    } = *ctx;
    match raw {
        ScalarRaw::Int(i) => Some(ConcreteValue::Int(*i)),
        ScalarRaw::Str(s) => {
            let parsed = match parse_value(s) {
                Ok(p) => p,
                Err(e) => {
                    out.push(Diagnostic::new(
                        codes::INVALID_VALUE,
                        ctx.loc.clone(),
                        format!(
                            "test `{test}` input `{group}`: invalid value `{s}` \
                             for parameter `{param}`: {e}"
                        ),
                    ));
                    return None;
                }
            };
            if let ValueKind::Var(name) = &parsed.kind {
                if let Some(value) = ctx.shared_singles.get(name) {
                    return Some(value.clone());
                }
                if ctx.merged.contains_key(name) {
                    out.push(Diagnostic::new(
                        codes::INVALID_VALUE,
                        ctx.loc.clone(),
                        format!(
                            "test `{test}` input `{group}`: `${name}` refers to \
                             shared parameter `{name}` which has multiple values"
                        ),
                    ));
                    return None;
                }
                out.push(Diagnostic::new(
                    codes::UNRESOLVED_VAR,
                    ctx.loc.clone(),
                    format!(
                        "test `{test}` input `{group}`: `${name}` in parameter \
                         `{param}` is not defined (own values may reference \
                         single-valued shared parameters via refs)"
                    ),
                ));
                return None;
            }
            concrete_of(parsed.kind, parsed.negated, ctx.loc, out)
        }
    }
}

/// An own-args range bound: resolved, then required to be an integer.
fn own_bound(raw: &ScalarRaw, ctx: &OwnCtx<'_>, out: &mut Vec<Diagnostic>) -> Option<i64> {
    match resolve_own_scalar(raw, ctx, out)? {
        ConcreteValue::Int(i) => Some(i),
        ConcreteValue::Str(s) => {
            out.push(Diagnostic::new(
                codes::INVALID_VALUE,
                ctx.loc.clone(),
                format!("range bound resolved to string `{s}`, must be an integer"),
            ));
            None
        }
    }
}

/// Converts a parsed literal into a concrete value, rejecting negation
/// outside expectation positions.
///
/// A `Var` here is a defensive branch: every caller intercepts variables
/// before delegating, so reaching it means an internal ordering mistake —
/// reported as an error, never a panic.
fn concrete_of(
    kind: ValueKind,
    negated: bool,
    loc: &Location,
    out: &mut Vec<Diagnostic>,
) -> Option<ConcreteValue> {
    if negated {
        out.push(Diagnostic::new(
            codes::INVALID_VALUE,
            loc.clone(),
            "negation `!` is only valid in expectation values".to_string(),
        ));
        return None;
    }
    match kind {
        ValueKind::Int(i) => Some(ConcreteValue::Int(i)),
        ValueKind::Str(s) => Some(ConcreteValue::Str(s)),
        ValueKind::Var(name) => {
            out.push(Diagnostic::new(
                codes::INVALID_VALUE,
                loc.clone(),
                format!("`{name}` was not resolved before literal conversion"),
            ));
            None
        }
    }
}

/// Enumerates a closed range `start..=end` by `step`; invariants are the
/// caller's responsibility. Addition is checked: an overflow ends the
/// enumeration instead of panicking.
fn enumerate_range(start: i64, end: i64, step: i64) -> Vec<ConcreteValue> {
    let mut values = Vec::new();
    let mut current = start;
    while current <= end {
        values.push(ConcreteValue::Int(current));
        match current.checked_add(step) {
            Some(next) => current = next,
            None => break,
        }
    }
    values
}

/// Resolves a test's commands against one sub-case binding.
///
/// Returns `None` when any value failed; the findings went to `agg`.
fn resolve_cmds(
    test: &str,
    group: Option<&str>,
    bindings: &BTreeMap<String, ConcreteValue>,
    cmds: &[Spanned<CmdDef>],
    doc: &SourceDoc,
    agg: &mut CmdErrorAggregator,
) -> Option<Vec<ResolvedCmd>> {
    agg.begin_subcase();
    let scope = match group {
        Some(g) => format!("test `{test}` input `{g}`"),
        None => format!("test `{test}`"),
    };

    let mut resolved = Vec::with_capacity(cmds.len());
    let mut ok = true;
    for (i, cmd) in cmds.iter().enumerate() {
        let cmd = cmd.get_ref();
        let loc = doc.locate(cmd.opfunc.span());
        let site = format!("{scope} command {i}");

        let mut args = Vec::with_capacity(cmd.args.len());
        for arg in &cmd.args {
            let entry = arg.get_ref();
            let Some((name, value_text)) = entry.split_once('=') else {
                // Validation reports the malformed entry; expansion
                // mirrors it so a standalone call cannot pass silently.
                agg.push(Diagnostic::new(
                    codes::INVALID_VALUE,
                    loc.clone(),
                    format!("{site}: argument `{entry}` is not in `name=value` form"),
                ));
                ok = false;
                continue;
            };
            match resolve_cmd_value(value_text, bindings) {
                Ok(value) => args.push((name.to_string(), value)),
                Err((code, message)) => {
                    agg.push(Diagnostic::new(
                        code,
                        loc.clone(),
                        format!("{site}: {message}"),
                    ));
                    ok = false;
                }
            }
        }

        let expect = {
            let assertions = cmd.registered_assertions();
            match assertions.as_slice() {
                [(assertion, raw)] => {
                    match resolve_expectation(*assertion, raw.get_ref(), bindings) {
                        Ok(e) => Some(e),
                        Err((code, message)) => {
                            agg.push(Diagnostic::new(
                                code,
                                loc.clone(),
                                format!("{site}: {message}"),
                            ));
                            ok = false;
                            None
                        }
                    }
                }
                // Validation enforces "exactly one" for test commands; env
                // commands are not resolved here.
                _ => None,
            }
        };

        resolved.push(ResolvedCmd {
            opfunc: cmd.opfunc.get_ref().clone(),
            args,
            expect,
            perf: cmd.perf,
            timeout: cmd.timeout,
        });
    }

    ok.then_some(resolved)
}

/// Resolves one command argument against the sub-case bindings.
fn resolve_cmd_value(
    text: &str,
    bindings: &BTreeMap<String, ConcreteValue>,
) -> Result<ConcreteValue, (&'static str, String)> {
    let parsed = parse_value(text)
        .map_err(|e| (codes::INVALID_VALUE, format!("invalid value `{text}`: {e}")))?;
    if parsed.negated {
        return Err((
            codes::INVALID_VALUE,
            "negation `!` is only valid in expectation values".to_string(),
        ));
    }
    match parsed.kind {
        ValueKind::Int(i) => Ok(ConcreteValue::Int(i)),
        ValueKind::Str(s) => Ok(ConcreteValue::Str(s)),
        ValueKind::Var(name) => bindings.get(&name).cloned().ok_or_else(|| {
            (
                codes::UNRESOLVED_VAR,
                format!("`${name}` is not defined in this sub-case"),
            )
        }),
    }
}

/// Resolves an expectation value, folding the FR-C-07 negation into the
/// final expectation kind: `expect_eq = "!7"` is `expect_ne = 7`.
///
/// The registered assertion declares how `!` folds it (its
/// [`Assertion::negated_field_name`]); a fold whose counterpart is not
/// registered is a load-time error, never a silent drop.
fn resolve_expectation(
    declared: &'static dyn crate::assertion::Assertion,
    raw: &ScalarRaw,
    bindings: &BTreeMap<String, ConcreteValue>,
) -> Result<ResolvedExpectation, (&'static str, String)> {
    let (value, value_negated) = match raw {
        ScalarRaw::Int(i) => (ConcreteValue::Int(*i), false),
        ScalarRaw::Str(s) => {
            let parsed = parse_value(s)
                .map_err(|e| (codes::INVALID_VALUE, format!("invalid value `{s}`: {e}")))?;
            let value = match parsed.kind {
                ValueKind::Int(i) => ConcreteValue::Int(i),
                ValueKind::Str(inner) => ConcreteValue::Str(inner),
                ValueKind::Var(name) => bindings.get(&name).cloned().ok_or_else(|| {
                    (
                        codes::UNRESOLVED_VAR,
                        format!("`${name}` is not defined in this sub-case"),
                    )
                })?,
            };
            (value, parsed.negated)
        }
    };
    let assertion = if value_negated {
        match declared.negated_field_name() {
            Some(name) => crate::assertion::lookup(name).ok_or_else(|| {
                (
                    codes::INVALID_VALUE,
                    format!(
                        "negation `!` on `{}` folds into `{name}`, which is not registered",
                        declared.field_name()
                    ),
                )
            })?,
            None => {
                return Err((
                    codes::INVALID_VALUE,
                    format!(
                        "negation `!` is not supported for `{}`",
                        declared.field_name()
                    ),
                ));
            }
        }
    } else {
        declared
    };
    Ok(ResolvedExpectation {
        kind: assertion.field_name(),
        value,
    })
}

/// Collects command-resolution errors, counting affected sub-cases.
///
/// The same problem — same code and message — recurs in every sub-case
/// of a group because the command text is static. The aggregator keeps
/// the first diagnostic and appends the number of further sub-cases
/// instead of repeating it (FR-C-08 "report once, completely").
#[derive(Default)]
struct CmdErrorAggregator {
    /// (code, message) -> index into `entries`/`counts`.
    index: HashMap<(&'static str, String), usize>,
    entries: Vec<Diagnostic>,
    counts: Vec<usize>,
    /// Keys already counted for the current sub-case.
    local: HashSet<(&'static str, String)>,
}

impl CmdErrorAggregator {
    /// Starts a new sub-case; identical errors within one sub-case count
    /// once.
    fn begin_subcase(&mut self) {
        self.local.clear();
    }

    /// Records one finding, or counts another affected sub-case.
    fn push(&mut self, diag: Diagnostic) {
        let key = (diag.code, diag.message.clone());
        if !self.local.insert(key.clone()) {
            return;
        }
        match self.index.get(&key) {
            Some(&i) => self.counts[i] += 1,
            None => {
                self.index.insert(key, self.entries.len());
                self.entries.push(diag);
                self.counts.push(1);
            }
        }
    }

    /// Emits the deduplicated findings into `out`.
    fn finish(self, out: &mut Vec<Diagnostic>) {
        for (mut diag, subcases) in self.entries.into_iter().zip(self.counts) {
            if subcases > 1 {
                diag.message
                    .push_str(&format!(" (also in {} more sub-cases)", subcases - 1));
            }
            out.push(diag);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::cases::CaseConfig;
    use super::*;

    /// Expands the first test of a case configuration.
    fn expand_first(text: &str) -> (Vec<SubCase>, Vec<Diagnostic>) {
        let doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: text.to_string(),
        };
        let config: CaseConfig = doc.parse().unwrap();
        let mut out = Vec::new();
        let subcases = expand_test(
            config.tests[0].get_ref(),
            &config.shared_inputs,
            &doc,
            &mut out,
        );
        (subcases, out)
    }

    fn codes_of(diags: &[Diagnostic]) -> Vec<&'static str> {
        diags.iter().map(|d| d.code).collect()
    }

    fn names_of(subcases: &[SubCase]) -> Vec<&str> {
        subcases.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn decision_q05_subcase_naming_format__F_C_05() {
        // The examples of requirement spec 7.3: a list parameter and a
        // closed range, both named per the Q-05 format with sorted keys.
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "test_rw_u32"
cmds = [{ opfunc = "Call_read32", expect_eq = "$val", args = ["addr_idx=1"] }]
[[tests.inputs]]
name = "ipt1"
refs = ["common"]
[shared_inputs.common]
val = ["888", "999"]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(
            names_of(&subcases),
            vec!["test_rw_u32/ipt1#0[val=888]", "test_rw_u32/ipt1#1[val=999]"]
        );

        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "test_range"
cmds = [{ opfunc = "Call_read32", expect_eq = "$addr", args = ["addr_idx=$addr"] }]
[[tests.inputs]]
name = "ipt1"
args = { addr = { start = 0, end = 8, step = 4 } }
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(
            names_of(&subcases),
            vec![
                "test_range/ipt1#0[addr=0]",
                "test_range/ipt1#1[addr=4]",
                "test_range/ipt1#2[addr=8]"
            ]
        );
    }

    #[test]
    fn first_sorted_param_varies_slowest__F_C_05() {
        // b (last sorted name) varies fastest; a stays put per outer step.
        let (subcases, _) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = [0, 1], b = [10, 20] }
"#,
        );
        assert_eq!(
            names_of(&subcases),
            vec![
                "t/g#0[a=0,b=10]",
                "t/g#1[a=0,b=20]",
                "t/g#2[a=1,b=10]",
                "t/g#3[a=1,b=20]"
            ]
        );
    }

    #[test]
    fn expansion_is_deterministic__F_C_05() {
        let text = r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = [1, 2], b = ["'x'", "'y'"], c = { start = 0, end = 2, step = 1 } }
"#;
        let (first_subcases, first_diags) = expand_first(text);
        let (second_subcases, _) = expand_first(text);
        assert!(
            first_diags.is_empty(),
            "unexpected diagnostics: {first_diags:?}"
        );
        let first = names_of(&first_subcases);
        let second = names_of(&second_subcases);
        assert_eq!(first, second);
        // 2 (a) x 2 (b) x 3 (c, closed range) concrete sub-cases.
        assert_eq!(first.len(), 12);
    }

    #[test]
    fn refs_merge_shared_params_into_group__F_C_06() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
[shared_inputs.common]
val = ["888", "999"]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(subcases.len(), 2);
        assert_eq!(subcases[0].bindings["val"], ConcreteValue::Int(888));
    }

    #[test]
    fn same_name_conflict_between_refs_is_reported__F_C_06() {
        let (_, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["one", "two"]
[shared_inputs.one]
val = 1
[shared_inputs.two]
val = 2
"#,
        );
        assert_eq!(codes_of(&diags), vec!["conflicting_input_param"]);
        assert!(
            diags[0].message.contains("shared_inputs.one")
                && diags[0].message.contains("shared_inputs.two"),
            "message names both sources: {}",
            diags[0].message
        );
    }

    #[test]
    fn conflict_between_ref_and_own_args_is_reported__F_C_06() {
        let (_, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
args = { val = 1 }
[shared_inputs.common]
val = 2
"#,
        );
        assert_eq!(codes_of(&diags), vec!["conflicting_input_param"]);
        assert!(
            diags[0].message.contains("the group's own args"),
            "message names the conflicting source: {}",
            diags[0].message
        );
    }

    #[test]
    fn shared_values_must_be_literals__F_C_06() {
        let (_, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
[shared_inputs.common]
val = "$other"
"#,
        );
        assert_eq!(codes_of(&diags), vec!["shared_not_literal"]);
    }

    #[test]
    fn var_in_list_resolves_against_shared_single__F_C_06() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
args = { x = ["$base"] }
[shared_inputs.common]
base = 100
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(subcases[0].bindings["x"], ConcreteValue::Int(100));
    }

    #[test]
    fn var_in_range_bound_resolves_against_shared_single__F_C_06() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
args = { addr = { start = "$base", end = 102, step = 1 } }
[shared_inputs.common]
base = 100
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        // The merged shared parameter `base` is a binding like any other,
        // so it appears in every sub-case name (FR-C-06, Q-05).
        assert_eq!(
            names_of(&subcases),
            vec![
                "t/g#0[addr=100,base=100]",
                "t/g#1[addr=101,base=100]",
                "t/g#2[addr=102,base=100]"
            ]
        );
    }

    #[test]
    fn var_referring_to_multivalued_shared_param_is_rejected__F_C_06() {
        let (_, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
args = { x = ["$vals"] }
[shared_inputs.common]
vals = [1, 2]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["invalid_value"]);
        assert!(diags[0].message.contains("multiple values"));
    }

    #[test]
    fn unresolved_var_in_group_args_is_reported__F_C_06() {
        let (_, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { x = ["$nope"] }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unresolved_var"]);
        assert!(
            diags[0].message.contains("$nope") && diags[0].message.contains("test `t`"),
            "message names the variable and the test: {}",
            diags[0].message
        );
    }

    #[test]
    fn cmd_var_resolves_per_subcase__F_C_05() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_read32", expect_eq = "$val", args = ["addr_idx=$val"] }]
[[tests.inputs]]
name = "g"
args = { val = [7, 9] }
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(subcases.len(), 2);
        assert_eq!(
            subcases[0].cmds[0].args,
            vec![("addr_idx".to_string(), ConcreteValue::Int(7))]
        );
        assert_eq!(
            subcases[0].cmds[0].expect,
            Some(ResolvedExpectation {
                kind: "expect_eq",
                value: ConcreteValue::Int(7)
            })
        );
        assert_eq!(
            subcases[1].cmds[0].expect,
            Some(ResolvedExpectation {
                kind: "expect_eq",
                value: ConcreteValue::Int(9)
            })
        );
    }

    #[test]
    fn expect_negation_folds_into_expectation_kind__F_C_07() {
        // expect_eq = "!7" is expect_ne = 7; expect_ne = "!7" is eq again.
        let (subcases, _) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [
  { opfunc = "Call_a", expect_eq = "!7" },
  { opfunc = "Call_b", expect_ne = "!7" },
  { opfunc = "Call_c", expect_ne = "0x10" },
]
"#,
        );
        let cmds = &subcases[0].cmds;
        assert_eq!(
            cmds[0].expect,
            Some(ResolvedExpectation {
                kind: "expect_ne",
                value: ConcreteValue::Int(7)
            })
        );
        assert_eq!(
            cmds[1].expect,
            Some(ResolvedExpectation {
                kind: "expect_eq",
                value: ConcreteValue::Int(7)
            })
        );
        assert_eq!(
            cmds[2].expect,
            Some(ResolvedExpectation {
                kind: "expect_ne",
                value: ConcreteValue::Int(16)
            })
        );
    }

    #[test]
    fn comparison_assertion_resolves_and_evaluates__F_V_03() {
        // The registry routes `expect_ge` through the same expansion path as
        // eq/ne, including `$var` substitution — no scheduler or config-model
        // change was needed to add it.
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_ge = "$floor" }]
[[tests.inputs]]
name = "g"
args = { floor = [7, 9] }
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(
            subcases[0].cmds[0].expect,
            Some(ResolvedExpectation {
                kind: "expect_ge",
                value: ConcreteValue::Int(7)
            })
        );
        let expectation = subcases[0].cmds[0].expect.as_ref().unwrap();
        assert!(expectation.evaluate(7).passed);
        assert!(expectation.evaluate(8).passed);
        assert!(!expectation.evaluate(6).passed);
        assert_eq!(expectation.evaluate(8).expectation, "expect_ge 7");
    }

    #[test]
    fn comparison_assertion_negation_folds_into_its_counterpart__F_V_03_E_02() {
        // E-02: `expect_ge` folds `!` into `expect_lt`, which is now
        // registered, so the fold resolves instead of failing.
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_ge = "!7" }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(
            subcases[0].cmds[0].expect,
            Some(ResolvedExpectation {
                kind: "expect_lt",
                value: ConcreteValue::Int(7)
            })
        );
        let expectation = subcases[0].cmds[0].expect.as_ref().unwrap();
        assert!(expectation.evaluate(6).passed);
        assert!(!expectation.evaluate(7).passed);
        assert!(!expectation.evaluate(8).passed);
        assert_eq!(expectation.evaluate(6).expectation, "expect_lt 7");
    }

    #[test]
    fn every_comparison_negation_fold_resolves__F_V_03_E_02() {
        // The full counterpart matrix folds cleanly: ge<->lt and gt<->le.
        // Each row names an actual value the folded assertion accepts.
        for (declared, folded, passing_actual) in [
            ("expect_ge", "expect_lt", 6i64),
            ("expect_gt", "expect_le", 6),
            ("expect_le", "expect_gt", 8),
            ("expect_lt", "expect_ge", 8),
        ] {
            let (subcases, diags) = expand_first(&format!(
                r#"
version = 1
[[tests]]
name = "t"
cmds = [{{ opfunc = "Call_ping", {declared} = "!7" }}]
"#
            ));
            assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
            let expect = subcases[0].cmds[0].expect.as_ref().unwrap();
            assert_eq!(expect.kind, folded);
            assert!(
                expect.evaluate(passing_actual).passed,
                "{folded} 7 must accept {passing_actual}"
            );
        }
    }

    #[test]
    fn negation_in_arg_position_is_rejected__F_C_07() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_a", expect_eq = 0, args = ["x=!7"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["invalid_value"]);
        assert!(subcases.is_empty());
    }

    #[test]
    fn unresolved_cmd_var_reports_test_and_command_once__F_C_06() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_a", expect_eq = "$ghost" }]
[[tests.inputs]]
name = "g"
args = { a = [1, 2, 3] }
"#,
        );
        // One finding, annotated with the two further sub-cases affected.
        assert_eq!(codes_of(&diags), vec!["unresolved_var"]);
        assert!(
            diags[0].message.contains("test `t`")
                && diags[0].message.contains("$ghost")
                && diags[0].message.contains("command 0"),
            "message names test, variable, and command: {}",
            diags[0].message
        );
        assert!(diags[0].message.contains("also in 2 more sub-cases"));
        assert!(subcases.is_empty());
    }

    #[test]
    fn hex_and_string_cmd_values_resolve__F_A_03() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [
  { opfunc = "Call_a", expect_eq = 0, args = ["byte=0x5A", "s='text'"] },
]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(
            subcases[0].cmds[0].args,
            vec![
                ("byte".to_string(), ConcreteValue::Int(0x5A)),
                ("s".to_string(), ConcreteValue::Str("text".to_string())),
            ]
        );
    }

    #[test]
    fn too_many_combinations_is_reported__F_C_04() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = { start = 0, end = 10000, step = 1 } }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["too_many_combinations"]);
        assert!(subcases.is_empty());
        assert!(diags[0].message.contains("10001"));
    }

    #[test]
    fn range_is_closed_interval__F_C_04() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = { start = 0, end = 8, step = 4 } }
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        // end=8 is included: the interval is closed.
        assert_eq!(
            names_of(&subcases),
            vec!["t/g#0[a=0]", "t/g#1[a=4]", "t/g#2[a=8]"]
        );
    }

    #[test]
    fn range_step_resolved_from_var_is_rechecked__F_C_04() {
        // c4 defers $var bounds to expansion; a bad resolved step must
        // still be caught here.
        let (_, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
args = { a = { start = "$zero", end = 8, step = "$zero" } }
[shared_inputs.common]
zero = 0
"#,
        );
        assert_eq!(codes_of(&diags), vec!["invalid_range"]);
    }

    #[test]
    fn test_without_inputs_yields_one_subcase__F_C_03() {
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(names_of(&subcases), vec!["t"]);
        assert!(subcases[0].bindings.is_empty());
        assert_eq!(subcases[0].cmds.len(), 1);
    }

    #[test]
    fn group_flags_override_the_subcase_defaults__F_C_05() {
        // Group-level should_panic/break_if_fail travel with every
        // sub-case the group expands into; a group that does not declare
        // them leaves `None`, which inherits the test-level flags.
        let (subcases, diags) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
should_panic = true
break_if_fail = false
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "overridden"
should_panic = false
break_if_fail = true
args = { a = [1, 2] }
[[tests.inputs]]
name = "inheriting"
args = { b = [3] }
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(subcases.len(), 3);
        assert_eq!(subcases[0].name, "t/overridden#0[a=1]");
        assert_eq!(subcases[0].should_panic, Some(false));
        assert_eq!(subcases[0].break_if_fail, Some(true));
        assert_eq!(subcases[1].name, "t/overridden#1[a=2]");
        assert_eq!(subcases[1].should_panic, Some(false));
        assert_eq!(subcases[1].break_if_fail, Some(true));
        assert_eq!(subcases[2].name, "t/inheriting#0[b=3]");
        assert_eq!(subcases[2].should_panic, None);
        assert_eq!(subcases[2].break_if_fail, None);
    }

    #[test]
    fn multiple_groups_expand_in_declaration_order__F_C_03() {
        let (subcases, _) = expand_first(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "second"
args = { b = [2] }
[[tests.inputs]]
name = "first"
args = { a = [1] }
"#,
        );
        assert_eq!(
            names_of(&subcases),
            vec!["t/second#0[b=2]", "t/first#0[a=1]"]
        );
    }
}
