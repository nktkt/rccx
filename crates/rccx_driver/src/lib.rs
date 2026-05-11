//! Pipeline driver for `rccx`.
//!
//! Phase 0 exposes only the option struct, the `--version` / `--help` /
//! `--explain` entry points, and a placeholder `compile` that emits a clean
//! "not yet implemented" diagnostic so we can wire CLI plumbing end-to-end.

pub mod options;

use std::path::PathBuf;

use rccx_diagnostics::code::{E_IO_FAILED, E_NO_INPUT, E_UNIMPLEMENTED};
use rccx_diagnostics::{render_human, Diagnostic, DiagnosticSink, Label};
use rccx_source::SourceMap;

pub use options::{CStandard, EmitKind, Options, SafeCMode};

/// Compiler version reported by `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `--help` text. Kept in one place so the CLI binary and the help skill stay
/// in sync.
pub const HELP_TEXT: &str = "\
rccx — Rust-implemented C compiler with optional Safe C mode

USAGE:
    rccx [OPTIONS] <INPUT>...

OPTIONS:
    -o <PATH>             Write output to <PATH>
    -std=<STD>            C standard (c89, c99, c11, c17, c23) [default: c17]
    -fsafe-c              Enable Safe C ownership / borrow / unsafe checks
    -fsafe-c=warn         Same as -fsafe-c, but emit warnings instead of errors
    -fno-safe-c           Disable Safe C checks (default)
    -emit=<KIND>          Emit intermediate form: tokens, ast, hir, mir, llvm-ir, obj, asm
    --explain <CODE>      Print the long-form explanation for a diagnostic code
    --json-diagnostics    Render diagnostics as JSON (reserved; not yet implemented)
    -h, --help            Print this help and exit
    -V, --version         Print version and exit

EXAMPLES:
    rccx hello.c -o hello
    rccx main.c -fsafe-c
    rccx --explain E0001
";

/// Result of running the compiler with a given option set.
#[derive(Debug)]
pub struct RunResult {
    pub sources: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
    pub success: bool,
}

impl RunResult {
    pub fn render_to_string(&self) -> String {
        let mut out = String::new();
        for d in &self.diagnostics {
            out.push_str(&render_human(d, &self.sources));
        }
        out
    }
}

/// Run the compiler pipeline with the given options.
///
/// Phase 0 only opens the input files (so we can report I/O errors with real
/// spans) and then emits a placeholder "not yet implemented" diagnostic.
pub fn compile(options: &Options) -> RunResult {
    let mut sink = DiagnosticSink::new();
    let mut sources = SourceMap::new();

    if options.inputs.is_empty() {
        sink.emit(
            Diagnostic::error(E_NO_INPUT, "no input files")
                .with_help("pass a C source file, e.g. `rccx hello.c`"),
        );
        return finish(sources, sink);
    }

    let mut any_file_loaded = false;
    for input in &options.inputs {
        match load_file(&mut sources, input) {
            Ok(_) => any_file_loaded = true,
            Err(diag) => sink.emit(diag),
        }
    }

    if any_file_loaded {
        sink.emit(
            Diagnostic::error(
                E_UNIMPLEMENTED,
                "compilation pipeline is not implemented yet (Phase 0)",
            )
            .with_note("Phase 0 only wires the CLI, source manager, and diagnostics")
            .with_help("see ROADMAP.md for the per-phase plan"),
        );
    }

    finish(sources, sink)
}

fn load_file(sources: &mut SourceMap, path: &PathBuf) -> Result<rccx_source::FileId, Diagnostic> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(sources.add_file(path.clone(), text)),
        Err(err) => Err(Diagnostic::error(
            E_IO_FAILED,
            format!("could not read `{}`: {err}", path.display()),
        )
        .with_label(Label::primary_unlabeled(rccx_source::Span::DUMMY))),
    }
}

fn finish(sources: SourceMap, mut sink: DiagnosticSink) -> RunResult {
    let success = !sink.has_errors();
    RunResult {
        sources,
        diagnostics: sink.drain(),
        success,
    }
}

/// `--explain <code>` output. Returns the long-form text or a structured
/// diagnostic when the code is unknown.
pub fn explain(code: &str) -> Result<String, Diagnostic> {
    match rccx_diagnostics::code::explain(code) {
        Some(entry) => Ok(format!(
            "{code} — {title}\n\n{body}\n",
            code = entry.code,
            title = entry.title,
            body = entry.body,
        )),
        None => Err(Diagnostic::bare_error(format!(
            "unknown diagnostic code `{code}`; run `rccx --explain E0001` for an example"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_inputs_produces_no_input_diagnostic() {
        let result = compile(&Options::default());
        assert!(!result.success);
        let s = result.render_to_string();
        assert!(s.contains("E9003"), "got: {s}");
        assert!(s.contains("no input files"), "got: {s}");
    }

    #[test]
    fn missing_file_reports_io_error() {
        let mut opts = Options::default();
        opts.inputs
            .push(PathBuf::from("/definitely/not/a/real/path.c"));
        let result = compile(&opts);
        assert!(!result.success);
        let s = result.render_to_string();
        assert!(s.contains("E9001"), "got: {s}");
        assert!(s.contains("could not read"), "got: {s}");
    }

    #[test]
    fn explain_known_code() {
        let out = explain("E0001").unwrap();
        assert!(out.starts_with("E0001 — use of moved owner pointer"));
    }

    #[test]
    fn explain_unknown_code() {
        let err = explain("E1234").unwrap_err();
        assert!(err.message.contains("unknown diagnostic code"));
    }
}
