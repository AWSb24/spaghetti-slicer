//! Errors and warnings with stable machine-readable codes.
//!
//! Every error type in the workspace implements [`Diagnose`], and every
//! warning is a [`Diagnostic`] collected in [`Diagnostics`]. Codes are
//! dotted lower-case names such as `model.truncated`; they are part of the
//! public interface and must not change once released.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The result was produced but may not be what the user expects.
    Warning,
    /// No result was produced.
    Error,
}

/// One error or warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// Whether this is a warning or an error.
    pub severity: Severity,
    /// Stable identifier, e.g. `settings.out_of_range`.
    pub code: &'static str,
    /// Human-readable description in English.
    pub message: String,
    /// Structured details such as the offending key, value or line.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<&'static str, String>,
}

impl Diagnostic {
    /// Creates a warning.
    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            context: BTreeMap::new(),
        }
    }

    /// Creates an error.
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            ..Self::warning(code, message)
        }
    }

    /// Adds a context entry.
    pub fn with(mut self, key: &'static str, value: impl ToString) -> Self {
        self.context.insert(key, value.to_string());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Implemented by every error type so callers can report it uniformly.
pub trait Diagnose: std::error::Error {
    /// Stable identifier of this error.
    fn code(&self) -> &'static str;

    /// Structured details of this error.
    fn context(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    /// The error as a [`Diagnostic`].
    fn diagnostic(&self) -> Diagnostic {
        let mut diagnostic = Diagnostic::error(self.code(), self.to_string());
        diagnostic.context.extend(self.context());
        diagnostic
    }
}

/// Collects warnings while an operation runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    /// An empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a diagnostic.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.0.push(diagnostic);
    }

    /// All diagnostics in the order they were reported.
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.0
    }

    /// Whether nothing was reported.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether a diagnostic with this code was reported.
    pub fn has(&self, code: &str) -> bool {
        self.0.iter().any(|d| d.code == code)
    }

    /// The codes of all diagnostics, in order.
    pub fn codes(&self) -> Vec<&'static str> {
        self.0.iter().map(|d| d.code).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Sample;
    impl fmt::Display for Sample {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "sample failed")
        }
    }
    impl std::error::Error for Sample {}
    impl Diagnose for Sample {
        fn code(&self) -> &'static str {
            "test.sample"
        }
        fn context(&self) -> Vec<(&'static str, String)> {
            vec![("line", "7".to_owned())]
        }
    }

    #[test]
    fn error_becomes_diagnostic() {
        let d = Sample.diagnostic();
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.code, "test.sample");
        assert_eq!(d.to_string(), "sample failed");
        assert_eq!(d.context["line"], "7");
    }

    #[test]
    fn collection() {
        let mut all = Diagnostics::new();
        assert!(all.is_empty());
        all.push(Diagnostic::warning("a.b", "first").with("n", 3));
        all.push(Diagnostic::warning("c.d", "second"));
        assert!(all.has("a.b") && !all.has("x.y"));
        assert_eq!(all.codes(), ["a.b", "c.d"]);
        assert_eq!(all.as_slice()[0].context["n"], "3");
    }
}
