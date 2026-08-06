//! The basic catalog's validation predicates, as pure functions.
//!
//! A2UI's `checks` are a list of `{condition, message}` rules, where the
//! condition is a `DynamicBoolean` — and the catalog's boolean functions
//! (`required`, `regex`, `length`, `numeric`, `email`, `and`, `or`, `not`)
//! are what a stream actually puts there. The leaves live here, evaluated
//! over already-resolved values; the composition (`and`/`or`/`not`, argument
//! resolution, data-model lookups, notes) needs render context and stays in
//! [`crate::render`], the same split [`crate::functions`] already uses.
//!
//! Where the semantics match, this defers to `fenestra_kit::validation`
//! rather than re-deriving them: the kit's engine already mirrors the web's
//! Constraint Validation API, which is the behavior an agent authoring for
//! a browser client expects. That includes the rule that **every predicate
//! but `required` passes on an empty value** — an optional field is valid
//! until it is filled in, exactly like HTML. A stream that wants a value
//! *and* wants it well-formed writes both checks, which is what a stream
//! written against a web client already does.

use fenestra_kit::validation::{Constraint, validate};
use serde_json::Value;

/// Compiles a validation pattern, or explains why it could not be.
///
/// A2UI patterns are written for browser clients, where the engine is
/// ECMAScript's — which has backreferences and lookaround that Rust's
/// linear-time engine deliberately does not. Such a pattern is not
/// malformed, it is unsupported *here*, and the difference matters to an
/// agent deciding whether to rewrite the pattern or the client. Either way
/// the caller must report it: a check that cannot be evaluated has to say
/// so, never quietly pass.
///
/// # Errors
/// Returns a [`PatternError`] when `pattern` does not compile.
pub fn compile_pattern(pattern: &str) -> Result<regex::Regex, PatternError> {
    regex::Regex::new(pattern).map_err(|e| {
        let reason = e.to_string();
        // The engine's own message is a multi-line diagram; the last
        // meaningful line is the part a note can carry.
        let message = reason
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("pattern did not compile")
            .trim()
            .to_owned();
        PatternError {
            // The engine says "not supported" for the constructs it
            // deliberately omits (look-around, backreferences) and gives an
            // ordinary syntax diagnostic for a pattern that is simply
            // wrong. Reporting the first as the second sends an agent to
            // rewrite its client; the second as the first sends it to
            // rewrite a client that was fine.
            unsupported_here: reason.contains("not supported"),
            message,
        }
    })
}

/// Why a validation pattern could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    /// The engine's diagnostic, trimmed to one line.
    pub message: String,
    /// True when the pattern is valid elsewhere and unsupported *here* —
    /// the constructs Rust's linear-time engine omits by design. False for
    /// a pattern that is malformed in any engine.
    pub unsupported_here: bool,
}

/// Whether `value` matches `pattern`, unanchored — the same reading a
/// browser's `RegExp.test` gives, so a stream authored against a web client
/// behaves identically here. A pattern that must match the whole value
/// anchors itself with `^…$`, as it would there.
#[must_use]
pub fn matches(re: &regex::Regex, value: &str) -> bool {
    // Empty stays exempt, like every other non-`required` predicate.
    value.trim().is_empty() || re.is_match(value)
}

/// Whether a value counts as provided.
///
/// Null, a whitespace-only string, and an empty list or object are all
/// "nothing here". So is `false`: `required` on a CheckBox is how a stream
/// says "you must accept the terms", and reading an unticked box as a
/// provided value would leave that impossible to express at all. A number
/// is a value, `0` included.
#[must_use]
pub fn required(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        Value::Number(_) => true,
    }
}

/// Whether a string is a syntactically valid email address.
#[must_use]
pub fn email(value: &str) -> bool {
    validate(value, &[Constraint::Email]).valid
}

/// Whether a string's length (in characters) is within `min..=max`.
///
/// Bounds are optional and independent, so `{"min": 8}` is a floor with no
/// ceiling. A bound past `usize` saturates: on a 32-bit target that keeps a
/// ceiling no string can exceed and a floor no string can reach, which is
/// what the stream asked for either way.
#[must_use]
pub fn length(value: &str, min: Option<u64>, max: Option<u64>) -> bool {
    let clamp = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
    let constraints: Vec<Constraint> = [
        min.map(|n| Constraint::MinLen(clamp(n))),
        max.map(|n| Constraint::MaxLen(clamp(n))),
    ]
    .into_iter()
    .flatten()
    .collect();
    validate(value, &constraints).valid
}

/// Whether a value is a finite number within `min..=max`.
///
/// A numeric string counts — a bound two-way input writes text, and a
/// stream that checks `numeric` on a text field means the text should be a
/// number. Anything that is not a finite number fails, which is the whole
/// point of the predicate; an empty value is exempt, like every other
/// non-`required` check.
#[must_use]
pub fn numeric(value: &Value, min: Option<f64>, max: Option<f64>) -> bool {
    let as_text = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        // A list or a bool is not a number and never becomes one; say so
        // rather than letting the empty-value exemption wave it through.
        _ => return false,
    };
    if as_text.trim().is_empty() {
        return true;
    }
    let mut constraints = vec![Constraint::Number];
    constraints.extend(min.map(Constraint::Min));
    constraints.extend(max.map(Constraint::Max));
    validate(&as_text, &constraints).valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn required_reads_an_unticked_box_as_nothing_provided() {
        assert!(!required(&json!(false)));
        assert!(required(&json!(true)));
    }

    #[test]
    fn required_accepts_zero_and_rejects_blank() {
        assert!(required(&json!(0)));
        assert!(required(&json!(0.0)));
        assert!(!required(&json!("")));
        assert!(!required(&json!("   ")));
        assert!(!required(&Value::Null));
        assert!(!required(&json!([])));
        assert!(!required(&json!({})));
        assert!(required(&json!(["a"])));
    }

    #[test]
    fn optional_checks_pass_on_an_empty_value() {
        assert!(email(""));
        assert!(length("", Some(8), None));
        assert!(numeric(&json!(""), Some(1.0), None));
        assert!(numeric(&Value::Null, Some(1.0), None));
    }

    #[test]
    fn email_shape() {
        assert!(email("ada@example.com"));
        assert!(!email("ada@@example.com"));
        assert!(!email("nope"));
    }

    #[test]
    fn length_bounds_are_independent_and_char_counted() {
        assert!(length("hello", Some(5), Some(5)));
        assert!(!length("hell", Some(5), None));
        assert!(!length("helloo", None, Some(5)));
        // Five characters, ten bytes — the limit counts characters.
        assert!(length("héllo", None, Some(5)));
    }

    #[test]
    fn an_enormous_length_bound_still_means_what_it_says() {
        // A ceiling nothing can exceed accepts everything…
        assert!(length("short", None, Some(u64::MAX)));
        // …and a floor nothing can reach accepts nothing. The stream asked
        // for something unsatisfiable; saying "invalid" is the honest
        // answer, not silently dropping the bound.
        assert!(!length("short", Some(u64::MAX), None));
    }

    #[test]
    fn numeric_accepts_numbers_and_numeric_text() {
        assert!(numeric(&json!(42), None, None));
        assert!(numeric(&json!("42"), None, None));
        assert!(numeric(&json!(42), Some(0.0), Some(100.0)));
        assert!(!numeric(&json!(101), None, Some(100.0)));
        assert!(!numeric(&json!("abc"), None, None));
    }

    #[test]
    fn a_pattern_matches_unanchored_and_exempts_empty() {
        let re = compile_pattern(r"\d{3}").expect("compiles");
        assert!(matches(&re, "abc123def"));
        assert!(!matches(&re, "ab"));
        assert!(matches(&re, ""), "an empty optional value is not invalid");

        let anchored = compile_pattern(r"^\d{3}$").expect("compiles");
        assert!(!matches(&anchored, "abc123def"));
        assert!(matches(&anchored, "123"));
    }

    #[test]
    fn an_ecmascript_only_pattern_reports_instead_of_compiling() {
        // Lookahead is valid in a browser and absent from Rust's engine.
        // The point is that this is an Err a caller must note, not a
        // silently-passing check.
        let err = compile_pattern(r"(?=.*[A-Z]).{8,}").expect_err("no lookaround in this engine");
        assert!(!err.message.is_empty(), "the note needs something to say");
        assert!(
            !err.message.contains('\n'),
            "a note is one line, got:\n{}",
            err.message
        );
        assert!(
            err.unsupported_here,
            "lookahead is valid in a browser; blaming the pattern would send an agent \
             to rewrite something that is fine"
        );
    }

    #[test]
    fn a_malformed_pattern_is_not_called_unsupported() {
        // Wrong in every engine, not missing from this one.
        for pattern in [r"[a-", r"(", r"a{2,1}"] {
            let err = compile_pattern(pattern).expect_err("malformed");
            assert!(
                !err.unsupported_here,
                "{pattern:?} is malformed, not unsupported: {}",
                err.message
            );
        }
    }

    #[test]
    fn a_list_is_never_a_number() {
        assert!(!numeric(&json!([1]), None, None));
        assert!(!numeric(&json!(true), None, None));
    }
}
