//! Diagnostics for `rccx`.
//!
//! A `Diagnostic` is the universal output of every compiler stage. Stages
//! produce diagnostics; the driver renders them. Stages never write to
//! stdout/stderr directly.
//!
//! The renderer in this crate is deliberately ASCII-only and deterministic
//! so the format is stable enough for golden snapshot tests.

pub mod code;
pub mod render;
pub mod sink;

use std::fmt;

use rccx_source::Span;

pub use code::{DiagnosticCode, ExplainEntry};
pub use render::render_human;
pub use sink::DiagnosticSink;

/// Severity of a diagnostic. Ordering matters: `error` aborts compilation,
/// `warning` does not, and `note` / `help` are decorative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Help => "help",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A span with an optional message. Multiple labels render together so users
/// can see related locations side by side.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: Option<String>,
    pub style: LabelStyle,
}

/// Whether a label is the primary one (caret `^`) or a secondary one (`-`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    Primary,
    Secondary,
}

impl Label {
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: Some(message.into()),
            style: LabelStyle::Primary,
        }
    }

    pub fn primary_unlabeled(span: Span) -> Self {
        Self {
            span,
            message: None,
            style: LabelStyle::Primary,
        }
    }

    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: Some(message.into()),
            style: LabelStyle::Secondary,
        }
    }
}

/// One compiler diagnostic.
///
/// Build with `Diagnostic::error(code, msg)` and chain `.with_*` methods.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<DiagnosticCode>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            helps: Vec::new(),
        }
    }

    pub fn error(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message).with_code(code)
    }

    pub fn warning(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message).with_code(code)
    }

    pub fn bare_error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    pub fn with_code(mut self, code: DiagnosticCode) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_primary(self, span: Span, message: impl Into<String>) -> Self {
        self.with_label(Label::primary(span, message))
    }

    pub fn with_secondary(self, span: Span, message: impl Into<String>) -> Self {
        self.with_label(Label::secondary(span, message))
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.helps.push(help.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}
