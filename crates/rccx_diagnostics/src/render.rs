//! Human-readable diagnostic renderer.
//!
//! Output is ASCII-only and deterministic so it can be compared verbatim in
//! golden tests. The format is intentionally close to rustc/clang:
//!
//! ```text
//! error[E0001]: use of moved owner pointer
//!   --> src/main.c:13:10
//!    |
//! 13 |     puts(name);
//!    |          ^^^^ value used after move
//!    = note: ownership was moved on line 11
//!    = help: pass a borrow instead
//! ```

use std::fmt::Write;

use rccx_source::{SourceMap, Span};

use crate::{Diagnostic, Label, LabelStyle};

/// Render a single diagnostic to a string.
pub fn render_human(diag: &Diagnostic, sources: &SourceMap) -> String {
    let mut out = String::new();
    render_header(&mut out, diag);
    render_labels(&mut out, diag, sources);
    render_footer(&mut out, diag);
    out
}

fn render_header(out: &mut String, diag: &Diagnostic) {
    let _ = write!(out, "{}", diag.severity);
    if let Some(code) = diag.code {
        let _ = write!(out, "[{code}]");
    }
    let _ = writeln!(out, ": {}", diag.message);
}

fn render_labels(out: &mut String, diag: &Diagnostic, sources: &SourceMap) {
    let primary = diag
        .labels
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .or_else(|| diag.labels.first());
    let Some(primary) = primary else { return };

    if primary.span.is_dummy() {
        return;
    }

    let Some(file) = sources.file(primary.span.file) else {
        return;
    };
    let start = file.line_col(primary.span.lo);
    let _ = writeln!(
        out,
        "  --> {}:{}:{}",
        file.path().display(),
        start.line,
        start.column,
    );

    // Group labels by line for compact rendering. Phase 0 keeps the format
    // simple: one block per labeled line, in input order.
    let gutter_width = compute_gutter_width(diag, sources);
    let pad = " ".repeat(gutter_width);
    let _ = writeln!(out, "{pad} |");

    for label in &diag.labels {
        render_label_block(out, label, sources, gutter_width);
    }
}

fn render_label_block(out: &mut String, label: &Label, sources: &SourceMap, gutter_width: usize) {
    if label.span.is_dummy() {
        return;
    }
    let Some(file) = sources.file(label.span.file) else {
        return;
    };
    let start = file.line_col(label.span.lo);
    let Some(line_text) = file.line_text(start.line) else {
        return;
    };

    let line_num = start.line.to_string();
    let pad_line = " ".repeat(gutter_width.saturating_sub(line_num.len()));
    let _ = writeln!(out, "{pad_line}{line_num} | {line_text}");

    let caret = caret_for(label, file, start.line);
    let pad = " ".repeat(gutter_width);
    if let Some(msg) = &label.message {
        let _ = writeln!(out, "{pad} | {caret} {msg}");
    } else {
        let _ = writeln!(out, "{pad} | {caret}");
    }
}

fn caret_for(label: &Label, file: &rccx_source::SourceFile, line: u32) -> String {
    let Some((line_start, line_end)) = file.line_range(line) else {
        return String::new();
    };
    let lo = label.span.lo.max(line_start);
    let hi = label.span.hi.min(line_end);
    let lead = file
        .text()
        .get(line_start as usize..lo as usize)
        .map(|s| s.chars().count())
        .unwrap_or(0);
    let span_text = file.text().get(lo as usize..hi as usize).unwrap_or("");
    let width = span_text.chars().count().max(1);

    let mark = match label.style {
        LabelStyle::Primary => '^',
        LabelStyle::Secondary => '-',
    };

    let mut s = " ".repeat(lead);
    for _ in 0..width {
        s.push(mark);
    }
    s
}

fn compute_gutter_width(diag: &Diagnostic, sources: &SourceMap) -> usize {
    let mut max_line: u32 = 1;
    for label in &diag.labels {
        if let Some(file) = sources.file(label.span.file) {
            let lc = file.line_col(label.span.lo);
            if lc.line > max_line {
                max_line = lc.line;
            }
        }
    }
    max_line.to_string().len()
}

fn render_footer(out: &mut String, diag: &Diagnostic) {
    let gutter = compute_footer_gutter(diag);
    let pad = " ".repeat(gutter);
    for note in &diag.notes {
        let _ = writeln!(out, "{pad} = note: {note}");
    }
    for help in &diag.helps {
        let _ = writeln!(out, "{pad} = help: {help}");
    }
}

fn compute_footer_gutter(diag: &Diagnostic) -> usize {
    // Match the gutter used by the labels even if we didn't render any.
    let mut width = 1;
    for label in &diag.labels {
        let n = label.span.hi.to_string().len();
        if n > width {
            width = n;
        }
    }
    width
}

/// Convenience: render a diagnostic with no source context. Useful for CLI
/// errors that happen before any file has been opened.
pub fn render_codeless(diag: &Diagnostic) -> String {
    let mut out = String::new();
    render_header(&mut out, diag);
    render_footer(&mut out, diag);
    out
}

/// Render the bare header for a span-less diagnostic.
pub fn header_for_span(_span: Span) -> &'static str {
    // Placeholder hook used by tests that want to assert on header shape
    // without invoking the full renderer.
    "  --> "
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{E_NO_INPUT, E_USE_AFTER_MOVE};
    use crate::{Diagnostic, Label};

    fn sm_one(text: &str) -> (SourceMap, rccx_source::FileId) {
        let mut sm = SourceMap::new();
        let id = sm.add_file("main.c", text);
        (sm, id)
    }

    #[test]
    fn header_includes_code_and_message() {
        let diag = Diagnostic::error(E_NO_INPUT, "no input files");
        let s = render_codeless(&diag);
        assert!(
            s.starts_with("error[E9003]: no input files\n"),
            "got: {s:?}"
        );
    }

    #[test]
    fn warning_severity_renders() {
        let diag = Diagnostic::warning(E_USE_AFTER_MOVE, "soft");
        let s = render_codeless(&diag);
        assert!(s.starts_with("warning[E0001]: soft\n"));
    }

    #[test]
    fn primary_label_has_caret() {
        let (sm, id) = sm_one("int x = 1;\nreturn x;\n");
        let span = rccx_source::Span::new(id, 4, 5);
        let diag =
            Diagnostic::error(E_USE_AFTER_MOVE, "boom").with_label(Label::primary(span, "here"));
        let s = render_human(&diag, &sm);
        let expected = "\
error[E0001]: boom
  --> main.c:1:5
  |
1 | int x = 1;
  |     ^ here
";
        assert_eq!(s, expected);
    }

    #[test]
    fn secondary_label_has_dash() {
        let (sm, id) = sm_one("a = b;\n");
        let span = rccx_source::Span::new(id, 4, 5);
        let diag =
            Diagnostic::error(E_USE_AFTER_MOVE, "x").with_label(Label::secondary(span, "context"));
        let s = render_human(&diag, &sm);
        assert!(s.contains("  | "), "got: {s:?}");
        assert!(s.contains("- context"), "got: {s:?}");
    }

    #[test]
    fn note_and_help_render_with_gutter() {
        let (sm, id) = sm_one("abc\n");
        let span = rccx_source::Span::new(id, 0, 1);
        let diag = Diagnostic::error(E_USE_AFTER_MOVE, "msg")
            .with_label(Label::primary(span, "p"))
            .with_note("first note")
            .with_help("first help");
        let s = render_human(&diag, &sm);
        assert!(s.contains("= note: first note"));
        assert!(s.contains("= help: first help"));
    }

    #[test]
    fn no_labels_renders_header_only() {
        let diag = Diagnostic::error(E_NO_INPUT, "no input");
        let sm = SourceMap::new();
        let s = render_human(&diag, &sm);
        assert_eq!(s, "error[E9003]: no input\n");
    }
}
