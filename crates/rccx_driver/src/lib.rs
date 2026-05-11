//! Pipeline driver for `rccx`.
//!
//! In Phase 0 the driver only opened input files and emitted a "not yet
//! implemented" diagnostic. Phase 1 adds the first real stage: lex the input
//! and, on `-emit=tokens`, dump the token stream.

pub mod options;

use std::fmt::Write as _;
use std::path::PathBuf;

use rccx_diagnostics::code::{E_IO_FAILED, E_NO_INPUT, E_UNIMPLEMENTED};
use rccx_diagnostics::{render_human, Diagnostic, DiagnosticSink, Label};
use rccx_lexer::{lex, Token, TokenKind};
use rccx_source::{SourceFile, SourceMap};

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
    /// Stdout-bound output produced by `-emit=...` modes. Empty in the
    /// regular compile path.
    pub emit: String,
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
/// Phase 1 covers `-emit=tokens`; later phases extend the pipeline. Any other
/// emit kind, and the regular compile path, still bottoms out at the
/// "not yet implemented" diagnostic.
pub fn compile(options: &Options) -> RunResult {
    let mut sink = DiagnosticSink::new();
    let mut sources = SourceMap::new();
    let mut emit = String::new();

    if options.inputs.is_empty() {
        sink.emit(
            Diagnostic::error(E_NO_INPUT, "no input files")
                .with_help("pass a C source file, e.g. `rccx hello.c`"),
        );
        return finish(sources, sink, emit);
    }

    let mut loaded_ids = Vec::new();
    for input in &options.inputs {
        match load_file(&mut sources, input) {
            Ok(id) => loaded_ids.push(id),
            Err(diag) => sink.emit(diag),
        }
    }

    match options.emit {
        Some(EmitKind::Tokens) => {
            for id in &loaded_ids {
                let file = sources.file(*id).expect("just loaded");
                let (tokens, diags) = lex(file);
                for diag in diags {
                    sink.emit(diag);
                }
                dump_tokens(&mut emit, file, &tokens);
            }
        }
        _ if !loaded_ids.is_empty() => {
            sink.emit(
                Diagnostic::error(
                    E_UNIMPLEMENTED,
                    "compilation pipeline beyond Phase 1 is not implemented yet",
                )
                .with_note("Phase 1 only wires lex + `-emit=tokens`; parser and codegen come later")
                .with_help("see ROADMAP.md for the per-phase plan"),
            );
        }
        _ => {}
    }

    finish(sources, sink, emit)
}

fn dump_tokens(out: &mut String, file: &SourceFile, tokens: &[Token]) {
    let _ = writeln!(out, "; tokens for {}", file.path().display());
    for tok in tokens {
        if matches!(tok.kind, TokenKind::Eof) {
            let _ = writeln!(out, "  [{:>4}..{:>4}) EOF", tok.span.lo, tok.span.hi);
            continue;
        }
        let text = file.slice(tok.span).unwrap_or("");
        let escaped = escape_for_dump(text);
        let _ = writeln!(
            out,
            "  [{:>4}..{:>4}) {:<18} {}",
            tok.span.lo,
            tok.span.hi,
            format_kind(tok.kind),
            escaped,
        );
    }
}

fn format_kind(kind: TokenKind) -> String {
    match kind {
        TokenKind::Keyword(kw) => format!("Keyword({})", kw.as_str()),
        TokenKind::IntLiteral { base } => format!("IntLiteral({base:?})"),
        other => format!("{other:?}"),
    }
}

fn escape_for_dump(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

fn finish(sources: SourceMap, mut sink: DiagnosticSink, emit: String) -> RunResult {
    let success = !sink.has_errors();
    RunResult {
        sources,
        diagnostics: sink.drain(),
        success,
        emit,
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
    fn emit_tokens_dumps_token_stream() {
        let dir = tempdir();
        let path = dir.join("hello.c");
        std::fs::write(&path, "int x = 42;\n").unwrap();
        let mut opts = Options::default();
        opts.inputs.push(path);
        opts.emit = Some(EmitKind::Tokens);
        let result = compile(&opts);
        assert!(result.success, "diagnostics: {}", result.render_to_string());
        assert!(result.emit.contains("Keyword(int)"), "{}", result.emit);
        assert!(result.emit.contains("Ident"), "{}", result.emit);
        assert!(
            result.emit.contains("IntLiteral(Decimal)"),
            "{}",
            result.emit
        );
        assert!(result.emit.contains("Semicolon"), "{}", result.emit);
        assert!(result.emit.ends_with("EOF\n"), "{}", result.emit);
    }

    fn tempdir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("rccx-test-{}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
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
