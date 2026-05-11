//! Pipeline driver for `rccx`.
//!
//! In Phase 0 the driver only opened input files and emitted a "not yet
//! implemented" diagnostic. Phase 1 adds the first real stage: lex the input
//! and, on `-emit=tokens`, dump the token stream.

pub mod options;

use std::fmt::Write as _;
use std::path::PathBuf;

use rccx_ast as ast;
use rccx_borrowck::{check as borrowck_check, BorrowCheckLevel};
use rccx_diagnostics::code::{E_IO_FAILED, E_NO_INPUT, E_UNIMPLEMENTED};
use rccx_diagnostics::{render_human, Diagnostic, DiagnosticSink, Label};
use rccx_hir as hir;
use rccx_lexer::{lex, Token, TokenKind};
use rccx_mir::{build_module as mir_build, dump as mir_dump};
use rccx_parser::parse_module;
use rccx_pp::{preprocess, PpOptions, UserDefine as PpUserDefine};
use rccx_source::{FileId, SourceFile, SourceMap, Span};
use rccx_typeck::check as typeck_check;

pub use options::{CStandard, EmitKind, Options, SafeCMode, UserDefine};

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
    -I <PATH>             Add <PATH> to the preprocessor include search list
    -D NAME[=BODY]        Predefine NAME as a preprocessor macro (default body: 1)
    -std=<STD>            C standard (c89, c99, c11, c17, c23) [default: c17]
    -fsafe-c              Enable Safe C ownership / borrow / unsafe checks
    -fsafe-c=warn         Same as -fsafe-c, but emit warnings instead of errors
    -fno-safe-c           Disable Safe C checks (default)
    -emit=<KIND>          Emit intermediate form:
                            tokens, pp-tokens, ast, hir, mir, llvm-ir, obj, asm
    --explain <CODE>      Print the long-form explanation for a diagnostic code
    --json-diagnostics    Render diagnostics as JSON (reserved; not yet implemented)
    -h, --help            Print this help and exit
    -V, --version         Print version and exit

EXAMPLES:
    rccx hello.c -o hello
    rccx main.c -fsafe-c
    rccx main.c -I include -DFOO=42 -emit=pp-tokens
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
        Some(EmitKind::PpTokens) => {
            let pp_opts = build_pp_options(options);
            for id in loaded_ids.clone() {
                let (tokens, diags) = preprocess(&mut sources, id, &pp_opts);
                for diag in diags {
                    sink.emit(diag);
                }
                dump_pp_tokens(&mut emit, &sources, id, &tokens);
            }
        }
        Some(EmitKind::Ast) => {
            let pp_opts = build_pp_options(options);
            for id in loaded_ids.clone() {
                let (tokens, pp_diags) = preprocess(&mut sources, id, &pp_opts);
                for d in pp_diags {
                    sink.emit(d);
                }
                let root_span = sources
                    .file(id)
                    .map(|f| Span::new(id, 0, f.text().len() as u32))
                    .unwrap_or(Span::DUMMY);
                let (module, parse_diags) = parse_module(&tokens, &sources, root_span);
                for d in parse_diags {
                    sink.emit(d);
                }
                let path = sources
                    .file(id)
                    .map(|f| f.path().display().to_string())
                    .unwrap_or_else(|| "?".to_string());
                let _ = writeln!(emit, "; ast for {path}");
                emit.push_str(&ast::dump(&module));
            }
        }
        Some(EmitKind::Hir) => {
            let pp_opts = build_pp_options(options);
            for id in loaded_ids.clone() {
                let (tokens, pp_diags) = preprocess(&mut sources, id, &pp_opts);
                for d in pp_diags {
                    sink.emit(d);
                }
                let root_span = sources
                    .file(id)
                    .map(|f| Span::new(id, 0, f.text().len() as u32))
                    .unwrap_or(Span::DUMMY);
                let (module, parse_diags) = parse_module(&tokens, &sources, root_span);
                for d in parse_diags {
                    sink.emit(d);
                }
                let (hir_module, tc_diags) = typeck_check(&module);
                for d in tc_diags {
                    sink.emit(d);
                }
                let path = sources
                    .file(id)
                    .map(|f| f.path().display().to_string())
                    .unwrap_or_else(|| "?".to_string());
                let _ = writeln!(emit, "; hir for {path}");
                emit.push_str(&hir::dump(&hir_module));
            }
        }
        Some(EmitKind::Mir) => {
            let pp_opts = build_pp_options(options);
            for id in loaded_ids.clone() {
                let (tokens, pp_diags) = preprocess(&mut sources, id, &pp_opts);
                for d in pp_diags {
                    sink.emit(d);
                }
                let root_span = sources
                    .file(id)
                    .map(|f| Span::new(id, 0, f.text().len() as u32))
                    .unwrap_or(Span::DUMMY);
                let (module, parse_diags) = parse_module(&tokens, &sources, root_span);
                for d in parse_diags {
                    sink.emit(d);
                }
                let (hir_module, tc_diags) = typeck_check(&module);
                for d in tc_diags {
                    sink.emit(d);
                }
                let mir = mir_build(&hir_module);
                let path = sources
                    .file(id)
                    .map(|f| f.path().display().to_string())
                    .unwrap_or_else(|| "?".to_string());
                let _ = writeln!(emit, "; mir for {path}");
                emit.push_str(&mir_dump(&mir));
            }
        }
        _ if !loaded_ids.is_empty() => {
            // Default (no -emit=...): run the full pipeline up through the
            // borrow checker if Safe C is active. Codegen is not yet wired.
            if options.safe_c != SafeCMode::Off {
                let pp_opts = build_pp_options(options);
                for id in loaded_ids.clone() {
                    let (tokens, pp_diags) = preprocess(&mut sources, id, &pp_opts);
                    for d in pp_diags {
                        sink.emit(d);
                    }
                    let root_span = sources
                        .file(id)
                        .map(|f| Span::new(id, 0, f.text().len() as u32))
                        .unwrap_or(Span::DUMMY);
                    let (module, parse_diags) = parse_module(&tokens, &sources, root_span);
                    for d in parse_diags {
                        sink.emit(d);
                    }
                    let (hir_module, tc_diags) = typeck_check(&module);
                    for d in tc_diags {
                        sink.emit(d);
                    }
                    let mir = mir_build(&hir_module);
                    let level = match options.safe_c {
                        SafeCMode::Off => unreachable!(),
                        SafeCMode::Error => BorrowCheckLevel::Error,
                        SafeCMode::Warn => BorrowCheckLevel::Warn,
                    };
                    for d in borrowck_check(&mir, level) {
                        sink.emit(d);
                    }
                }
            } else {
                sink.emit(
                    Diagnostic::error(E_UNIMPLEMENTED, "code generation is not implemented yet")
                        .with_note(
                            "Phase 7 MVP wires lex + preprocessor + parser + typeck + MIR + \
                         borrow checker (via `-fsafe-c`); LLVM codegen lands in Phase 8",
                        )
                        .with_help(
                            "try `-emit=mir` for now, or add `-fsafe-c` to run the borrow checker",
                        ),
                );
            }
        }
        _ => {}
    }

    finish(sources, sink, emit)
}

fn build_pp_options(options: &Options) -> PpOptions {
    PpOptions {
        include_paths: options.include_paths.clone(),
        user_defines: options
            .user_defines
            .iter()
            .map(|d| PpUserDefine {
                name: d.name.clone(),
                body: d.body.clone(),
            })
            .collect(),
    }
}

fn dump_pp_tokens(out: &mut String, sources: &SourceMap, root: FileId, tokens: &[Token]) {
    let root_path = sources
        .file(root)
        .map(|f| f.path().display().to_string())
        .unwrap_or_else(|| "?".to_string());
    let _ = writeln!(out, "; pp-tokens for {root_path}");
    for tok in tokens {
        if matches!(tok.kind, TokenKind::Eof) {
            let _ = writeln!(out, "  [{:>4}..{:>4}) EOF", tok.span.lo, tok.span.hi);
            continue;
        }
        let text = sources
            .file(tok.span.file)
            .and_then(|f| f.slice(tok.span))
            .unwrap_or("");
        let file_name = sources
            .file(tok.span.file)
            .map(|f| {
                f.path()
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| f.path().display().to_string())
            })
            .unwrap_or_else(|| "?".to_string());
        let _ = writeln!(
            out,
            "  {file_name}[{:>4}..{:>4}) {:<18} {}",
            tok.span.lo,
            tok.span.hi,
            format_kind(tok.kind),
            escape_for_dump(text),
        );
    }
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
        let dir = tempdir("emit-tokens");
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

    #[test]
    fn fsafec_detects_double_consume_of_owner() {
        let dir = tempdir("fsafe-c");
        let path = dir.join("uaf.c");
        std::fs::write(
            &path,
            "void consume([[sc::owner]] int *p);\n\
             void f(void) {\n\
                 [[sc::owner]] int *p;\n\
                 consume(p);\n\
                 consume(p);\n\
             }\n",
        )
        .unwrap();
        let mut opts = Options::default();
        opts.inputs.push(path);
        opts.safe_c = SafeCMode::Error;
        let result = compile(&opts);
        let rendered = result.render_to_string();
        assert!(!result.success, "{rendered}");
        assert!(rendered.contains("E0001"), "{rendered}");
        assert!(
            rendered.contains("use of moved owner pointer"),
            "{rendered}"
        );
    }

    #[test]
    fn fsafec_off_does_not_diagnose() {
        let dir = tempdir("fsafe-c-off");
        let path = dir.join("uaf.c");
        std::fs::write(
            &path,
            "void consume([[sc::owner]] int *p);\n\
             void f(void) {\n\
                 [[sc::owner]] int *p;\n\
                 consume(p);\n\
                 consume(p);\n\
             }\n",
        )
        .unwrap();
        let mut opts = Options::default();
        opts.inputs.push(path);
        // safe_c stays Off; the driver should still complain that codegen
        // isn't implemented but should NOT emit E0001.
        let result = compile(&opts);
        let rendered = result.render_to_string();
        assert!(!rendered.contains("E0001"), "{rendered}");
    }

    #[test]
    fn emit_mir_builds_bodies() {
        let dir = tempdir("emit-mir");
        let path = dir.join("hello.c");
        std::fs::write(
            &path,
            "int main(void) { int x = 1; if (x) { return 2; } return 0; }\n",
        )
        .unwrap();
        let mut opts = Options::default();
        opts.inputs.push(path);
        opts.emit = Some(EmitKind::Mir);
        let result = compile(&opts);
        assert!(result.success, "diagnostics: {}", result.render_to_string());
        assert!(result.emit.contains("fn `main`"), "{}", result.emit);
        assert!(result.emit.contains("switchInt"), "{}", result.emit);
        assert!(result.emit.contains("return"), "{}", result.emit);
    }

    #[test]
    fn emit_hir_runs_typeck() {
        let dir = tempdir("emit-hir");
        let path = dir.join("hello.c");
        std::fs::write(&path, "int add(int a, int b) { return a + b; }\n").unwrap();
        let mut opts = Options::default();
        opts.inputs.push(path);
        opts.emit = Some(EmitKind::Hir);
        let result = compile(&opts);
        assert!(result.success, "diagnostics: {}", result.render_to_string());
        assert!(result.emit.contains("FnDef `add`"), "{}", result.emit);
        assert!(result.emit.contains("Binary Add : int"), "{}", result.emit);
        assert!(result.emit.contains("Ref `a`"), "{}", result.emit);
    }

    #[test]
    fn emit_ast_runs_parser() {
        let dir = tempdir("emit-ast");
        let path = dir.join("hello.c");
        std::fs::write(&path, "int main(void) { return 0; }\n").unwrap();
        let mut opts = Options::default();
        opts.inputs.push(path);
        opts.emit = Some(EmitKind::Ast);
        let result = compile(&opts);
        assert!(result.success, "diagnostics: {}", result.render_to_string());
        assert!(result.emit.contains("FnDef `main`"), "{}", result.emit);
        assert!(result.emit.contains("Return"), "{}", result.emit);
        assert!(result.emit.contains("IntLit 0"), "{}", result.emit);
    }

    #[test]
    fn emit_pp_tokens_runs_preprocessor() {
        let dir = tempdir("emit-pp-tokens");
        let header = dir.join("h.h");
        std::fs::write(&header, "int y;\n").unwrap();
        let main = dir.join("main.c");
        std::fs::write(&main, "#define N 7\n#include \"h.h\"\nint x = N;\n").unwrap();
        let mut opts = Options::default();
        opts.inputs.push(main);
        opts.emit = Some(EmitKind::PpTokens);
        let result = compile(&opts);
        assert!(result.success, "diagnostics: {}", result.render_to_string());
        assert!(result.emit.contains("Keyword(int)"), "{}", result.emit);
        assert!(
            result.emit.contains("IntLiteral(Decimal) \"7\""),
            "{}",
            result.emit
        );
        // The header's identifier `y` shows up from h.h.
        assert!(result.emit.contains("h.h["), "{}", result.emit);
        assert!(result.emit.ends_with("EOF\n"), "{}", result.emit);
    }

    fn tempdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        let id = N.fetch_add(1, Ordering::Relaxed);
        p.push(format!("rccx-driver-{}-{}-{}", tag, std::process::id(), id));
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
