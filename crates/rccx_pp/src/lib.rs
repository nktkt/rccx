//! C preprocessor for `rccx` — Phase 2 MVP.
//!
//! Supports:
//!
//! - `#include "..."` and `#include <...>` with `-I` search paths.
//! - Object-like `#define NAME ...` and `#undef NAME`.
//! - Identifier replacement with a self-recursion guard.
//! - Stripping of whitespace, newline, and comment tokens from the output.
//!
//! Out of scope for the MVP (planned for later in Phase 2):
//!
//! - Function-like macros.
//! - Conditional compilation (`#if`, `#ifdef`, `#elif`, `#else`, `#endif`).
//! - `#` (stringize) and `##` (token-paste) operators.
//! - Predefined macros (`__FILE__`, `__LINE__`, `__STDC__`, ...).
//! - `#error`, `#warning`, `#pragma`, `#line`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rccx_diagnostics::code::{DiagnosticCode, E_IO_FAILED};
use rccx_diagnostics::{Diagnostic, Label};
use rccx_lexer::{lex, Token, TokenKind};
use rccx_source::{FileId, SourceMap, Span};

// === Diagnostic codes owned by the preprocessor ============================

pub const E_PP_UNKNOWN_DIRECTIVE: DiagnosticCode = DiagnosticCode("E0201");
pub const E_PP_BAD_INCLUDE: DiagnosticCode = DiagnosticCode("E0202");
pub const E_PP_INCLUDE_NOT_FOUND: DiagnosticCode = DiagnosticCode("E0203");
pub const E_PP_INCLUDE_CYCLE: DiagnosticCode = DiagnosticCode("E0204");
pub const E_PP_BAD_DEFINE: DiagnosticCode = DiagnosticCode("E0205");
pub const E_PP_MAX_DEPTH: DiagnosticCode = DiagnosticCode("E0206");

/// Maximum macro expansion depth before the preprocessor gives up. Catches
/// pathological mutually-recursive macros even when the per-name guard would
/// in principle let them through.
const MAX_EXPANSION_DEPTH: usize = 64;

/// Maximum `#include` nesting depth.
const MAX_INCLUDE_DEPTH: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct PpOptions {
    /// `-I` search paths, in priority order.
    pub include_paths: Vec<PathBuf>,
    /// `-D NAME[=BODY]` definitions seeded before processing starts.
    pub user_defines: Vec<UserDefine>,
}

#[derive(Debug, Clone)]
pub struct UserDefine {
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone)]
struct Macro {
    /// Replacement tokens. Trivia has already been stripped.
    replacement: Vec<Token>,
}

/// Run the preprocessor on `root_file` and return the cleaned-up token stream.
///
/// `sources` is consulted to look up file text and is mutated to load new
/// files discovered through `#include`. The returned token vector ends with
/// a synthetic [`TokenKind::Eof`] whose span lives in `root_file`.
pub fn preprocess(
    sources: &mut SourceMap,
    root_file: FileId,
    options: &PpOptions,
) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut pp = Preprocessor {
        sources,
        options,
        macros: HashMap::new(),
        diagnostics: Vec::new(),
        include_stack: Vec::new(),
    };

    for ud in &options.user_defines {
        pp.add_user_define(ud);
    }

    let mut output = Vec::new();
    pp.process_file(root_file, &mut output);

    let eof_pos = pp
        .sources
        .file(root_file)
        .map(|f| f.text().len() as u32)
        .unwrap_or(0);
    output.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(root_file, eof_pos, eof_pos),
    });

    (output, pp.diagnostics)
}

struct Preprocessor<'a> {
    sources: &'a mut SourceMap,
    options: &'a PpOptions,
    macros: HashMap<String, Macro>,
    diagnostics: Vec<Diagnostic>,
    include_stack: Vec<FileId>,
}

impl<'a> Preprocessor<'a> {
    fn add_user_define(&mut self, ud: &UserDefine) {
        // Lex the body as if it were source. We give it a virtual file so
        // spans in the replacement still resolve to readable text in
        // diagnostics.
        let virt = self
            .sources
            .add_file(format!("<command-line:-D{}>", ud.name), ud.body.clone());
        let file = self.sources.file(virt).expect("just added");
        let (tokens, lex_diags) = lex(file);
        self.diagnostics.extend(lex_diags);
        let replacement = strip_trivia_and_eof(tokens);
        self.macros.insert(ud.name.clone(), Macro { replacement });
    }

    fn process_file(&mut self, file_id: FileId, out: &mut Vec<Token>) {
        if self.include_stack.contains(&file_id) {
            let path = self
                .sources
                .file(file_id)
                .map(|f| f.path().display().to_string())
                .unwrap_or_else(|| "?".to_string());
            self.diagnostics.push(
                Diagnostic::error(
                    E_PP_INCLUDE_CYCLE,
                    format!("`#include` cycle detected at `{path}`"),
                )
                .with_help("break the cycle, or add an include guard"),
            );
            return;
        }
        if self.include_stack.len() >= MAX_INCLUDE_DEPTH {
            self.diagnostics.push(Diagnostic::error(
                E_PP_MAX_DEPTH,
                format!("`#include` nesting exceeds {MAX_INCLUDE_DEPTH} levels"),
            ));
            return;
        }

        self.include_stack.push(file_id);

        let file = self.sources.file(file_id).expect("file loaded");
        let (tokens, lex_diags) = lex(file);
        self.diagnostics.extend(lex_diags);

        let mut i = 0;
        while i < tokens.len() {
            let line_start = i;
            let mut line_end = i;
            while line_end < tokens.len()
                && !matches!(tokens[line_end].kind, TokenKind::Newline | TokenKind::Eof)
            {
                line_end += 1;
            }

            let line = &tokens[line_start..line_end];
            let mut first_real = 0;
            while first_real < line.len() && line[first_real].kind.is_trivia() {
                first_real += 1;
            }

            if first_real < line.len() && line[first_real].kind == TokenKind::Hash {
                self.handle_directive(&line[first_real + 1..], file_id, out);
            } else {
                self.emit_line(line, file_id, out);
            }

            // Step past the line terminator.
            i = line_end + 1;
            if i > tokens.len() {
                i = tokens.len();
            }
        }

        self.include_stack.pop();
    }

    fn handle_directive(&mut self, body: &[Token], file_id: FileId, out: &mut Vec<Token>) {
        // First non-trivia token names the directive.
        let mut idx = 0;
        while idx < body.len() && body[idx].kind.is_trivia() {
            idx += 1;
        }
        let Some(name_tok) = body.get(idx) else {
            // `#` on its own — null directive. Standard C accepts it.
            return;
        };
        if name_tok.kind != TokenKind::Ident {
            self.diagnostics.push(
                Diagnostic::error(
                    E_PP_UNKNOWN_DIRECTIVE,
                    "expected a directive name after `#`",
                )
                .with_label(Label::primary_unlabeled(name_tok.span)),
            );
            return;
        }
        let name = self.token_text(*name_tok).to_string();
        let rest = &body[idx + 1..];
        match name.as_str() {
            "include" => self.handle_include(rest, file_id, out),
            "define" => self.handle_define(rest),
            "undef" => self.handle_undef(rest),
            _ => {
                self.diagnostics.push(
                    Diagnostic::error(
                        E_PP_UNKNOWN_DIRECTIVE,
                        format!("unknown preprocessor directive `#{name}`"),
                    )
                    .with_label(Label::primary_unlabeled(name_tok.span))
                    .with_note("Phase 2 MVP only handles `#include`, `#define`, and `#undef`"),
                );
            }
        }
    }

    fn handle_include(&mut self, rest: &[Token], file_id: FileId, out: &mut Vec<Token>) {
        // Skip leading trivia.
        let mut idx = 0;
        while idx < rest.len() && rest[idx].kind.is_trivia() {
            idx += 1;
        }
        let Some(first) = rest.get(idx) else {
            self.diagnostics.push(Diagnostic::error(
                E_PP_BAD_INCLUDE,
                "`#include` requires a header name",
            ));
            return;
        };

        // We re-scan the source text directly: token boundaries inside `<...>`
        // are not reliable (`<stdio.h>` lexes as Lt/Ident/Dot/Ident/Gt), so
        // grab the raw slice from `first.span.lo` to the end of the line.
        let file = self.sources.file(file_id).expect("processing file");
        let (line_start, line_end) = file
            .line_range(file.line_col(first.span.lo).line)
            .expect("span in file");
        let raw = &file.text()[first.span.lo as usize..line_end as usize];
        let trimmed = raw.trim();
        let (header, is_system) = match trimmed.as_bytes().first().copied() {
            Some(b'"') => match trimmed[1..].find('"') {
                Some(end) => (&trimmed[1..1 + end], false),
                None => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            E_PP_BAD_INCLUDE,
                            "unterminated header name in `#include`",
                        )
                        .with_label(Label::primary_unlabeled(first.span)),
                    );
                    return;
                }
            },
            Some(b'<') => match trimmed[1..].find('>') {
                Some(end) => (&trimmed[1..1 + end], true),
                None => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            E_PP_BAD_INCLUDE,
                            "unterminated header name in `#include`",
                        )
                        .with_label(Label::primary_unlabeled(first.span)),
                    );
                    return;
                }
            },
            _ => {
                self.diagnostics.push(
                    Diagnostic::error(E_PP_BAD_INCLUDE, "`#include` expects `\"...\"` or `<...>`")
                        .with_label(Label::primary_unlabeled(first.span)),
                );
                return;
            }
        };
        let _ = line_start;

        let header = header.to_string();
        let span = first.span;
        match self.resolve_include(&header, is_system, file_id) {
            Some(included_id) => {
                let mut nested = Vec::new();
                self.process_file(included_id, &mut nested);
                // `process_file` appends EOF only at the top level via
                // `preprocess`, not from inside the loop, so nested already
                // has the right shape (trivia-free, no synthetic EOF).
                out.extend(nested);
            }
            None => {
                self.diagnostics.push(
                    Diagnostic::error(
                        E_PP_INCLUDE_NOT_FOUND,
                        format!("cannot find header `{header}`"),
                    )
                    .with_label(Label::primary_unlabeled(span))
                    .with_help("check `-I` paths and that the file exists"),
                );
            }
        }
    }

    fn handle_define(&mut self, rest: &[Token]) {
        let mut idx = 0;
        while idx < rest.len() && rest[idx].kind.is_trivia() {
            idx += 1;
        }
        let Some(name_tok) = rest.get(idx) else {
            self.diagnostics.push(Diagnostic::error(
                E_PP_BAD_DEFINE,
                "`#define` requires a macro name",
            ));
            return;
        };
        if name_tok.kind != TokenKind::Ident {
            self.diagnostics.push(
                Diagnostic::error(
                    E_PP_BAD_DEFINE,
                    "`#define` macro name must be an identifier",
                )
                .with_label(Label::primary_unlabeled(name_tok.span)),
            );
            return;
        }
        let name = self.token_text(*name_tok).to_string();
        let body = &rest[idx + 1..];
        let replacement = strip_trivia_and_eof(body.to_vec());
        self.macros.insert(name, Macro { replacement });
    }

    fn handle_undef(&mut self, rest: &[Token]) {
        let mut idx = 0;
        while idx < rest.len() && rest[idx].kind.is_trivia() {
            idx += 1;
        }
        let Some(name_tok) = rest.get(idx) else {
            self.diagnostics.push(Diagnostic::error(
                E_PP_BAD_DEFINE,
                "`#undef` requires a macro name",
            ));
            return;
        };
        if name_tok.kind != TokenKind::Ident {
            self.diagnostics.push(
                Diagnostic::error(E_PP_BAD_DEFINE, "`#undef` macro name must be an identifier")
                    .with_label(Label::primary_unlabeled(name_tok.span)),
            );
            return;
        }
        let name = self.token_text(*name_tok).to_string();
        self.macros.remove(&name);
    }

    fn emit_line(&mut self, line: &[Token], _file_id: FileId, out: &mut Vec<Token>) {
        for tok in line {
            if tok.kind.is_trivia() {
                continue;
            }
            if tok.kind == TokenKind::Ident {
                let name = self.token_text(*tok).to_string();
                let mut active = HashSet::new();
                self.expand_ident(&name, *tok, &mut active, 0, out);
            } else {
                out.push(*tok);
            }
        }
    }

    fn expand_ident(
        &mut self,
        name: &str,
        site: Token,
        active: &mut HashSet<String>,
        depth: usize,
        out: &mut Vec<Token>,
    ) {
        if depth > MAX_EXPANSION_DEPTH {
            self.diagnostics.push(
                Diagnostic::error(
                    E_PP_MAX_DEPTH,
                    format!("macro expansion of `{name}` exceeded depth limit"),
                )
                .with_label(Label::primary_unlabeled(site.span)),
            );
            out.push(site);
            return;
        }
        if active.contains(name) {
            out.push(site);
            return;
        }
        let Some(m) = self.macros.get(name).cloned() else {
            out.push(site);
            return;
        };
        active.insert(name.to_string());
        for tok in &m.replacement {
            if tok.kind == TokenKind::Ident {
                let sub = self.token_text(*tok).to_string();
                self.expand_ident(&sub, *tok, active, depth + 1, out);
            } else {
                out.push(*tok);
            }
        }
        active.remove(name);
    }

    fn resolve_include(
        &mut self,
        header: &str,
        is_system: bool,
        current: FileId,
    ) -> Option<FileId> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if !is_system {
            if let Some(curr_dir) = self
                .sources
                .file(current)
                .and_then(|f| f.path().parent().map(Path::to_path_buf))
            {
                candidates.push(curr_dir.join(header));
            }
        }
        for inc in &self.options.include_paths {
            candidates.push(inc.join(header));
        }

        for path in candidates {
            // Avoid loading the same file twice; reuse FileId if already
            // present in the source map.
            if let Some(existing) = self
                .sources
                .files()
                .find(|f| f.path() == path)
                .map(|f| f.id())
            {
                return Some(existing);
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => return Some(self.sources.add_file(path, text)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    self.diagnostics.push(Diagnostic::error(
                        E_IO_FAILED,
                        format!("could not read `{}`: {err}", path.display()),
                    ));
                    return None;
                }
            }
        }
        None
    }

    fn token_text(&self, tok: Token) -> &str {
        self.sources
            .file(tok.span.file)
            .and_then(|f| f.slice(tok.span))
            .unwrap_or("")
    }
}

fn strip_trivia_and_eof(tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .filter(|t| !t.kind.is_trivia() && t.kind != TokenKind::Eof)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(prefix: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        p.push(format!("rccx-pp-{}-{}-{}", prefix, std::process::id(), n));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn pp_str(text: &str) -> (Vec<Token>, Vec<Diagnostic>, SourceMap) {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", text);
        let (toks, diags) = preprocess(&mut sm, id, &PpOptions::default());
        (toks, diags, sm)
    }

    fn text_of(sm: &SourceMap, tok: &Token) -> String {
        sm.file(tok.span.file)
            .and_then(|f| f.slice(tok.span))
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn empty_file_yields_just_eof() {
        let (toks, diags, _) = pp_str("");
        assert!(diags.is_empty());
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::Eof);
    }

    #[test]
    fn passthrough_strips_trivia() {
        let (toks, diags, sm) = pp_str("int  x  ;\n");
        assert!(diags.is_empty());
        let texts: Vec<_> = toks.iter().map(|t| text_of(&sm, t)).collect();
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(texts, vec!["int", "x", ";", ""]);
        assert!(matches!(kinds[0], TokenKind::Keyword(_)));
        assert_eq!(kinds[1], TokenKind::Ident);
        assert_eq!(kinds[2], TokenKind::Semicolon);
        assert_eq!(kinds[3], TokenKind::Eof);
    }

    #[test]
    fn object_like_define_expands() {
        let src = "#define N 42\nint x = N;\n";
        let (toks, diags, sm) = pp_str(src);
        assert!(diags.is_empty());
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        assert_eq!(texts, vec!["int", "x", "=", "42", ";"]);
    }

    #[test]
    fn undef_removes_macro() {
        let src = "#define N 42\n#undef N\nint x = N;\n";
        let (toks, diags, sm) = pp_str(src);
        assert!(diags.is_empty());
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        // N is no longer defined; it stays as an identifier.
        assert_eq!(texts, vec!["int", "x", "=", "N", ";"]);
    }

    #[test]
    fn redefine_overwrites() {
        let src = "#define N 1\n#define N 2\nint x = N;\n";
        let (toks, _, sm) = pp_str(src);
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        assert_eq!(texts, vec!["int", "x", "=", "2", ";"]);
    }

    #[test]
    fn nested_object_like_expansion() {
        let src = "#define A B\n#define B C\n#define C 9\nA\n";
        let (toks, diags, sm) = pp_str(src);
        assert!(diags.is_empty());
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        assert_eq!(texts, vec!["9"]);
    }

    #[test]
    fn self_recursive_macro_does_not_loop() {
        // Classic C self-reference: x stays as identifier after one expansion.
        let src = "#define x x + 1\nx\n";
        let (toks, diags, sm) = pp_str(src);
        assert!(diags.is_empty(), "{diags:?}");
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        assert_eq!(texts, vec!["x", "+", "1"]);
    }

    #[test]
    fn unknown_directive_is_diagnosed() {
        let (_, diags, _) = pp_str("#nonsense\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0201");
    }

    #[test]
    fn null_directive_is_silent() {
        let (toks, diags, _) = pp_str("#\nint x;\n");
        assert!(diags.is_empty());
        // Only `int x ;` survives plus EOF.
        assert_eq!(toks.iter().filter(|t| t.kind != TokenKind::Eof).count(), 3);
    }

    #[test]
    fn define_without_name_is_diagnosed() {
        let (_, diags, _) = pp_str("#define\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0205");
    }

    #[test]
    fn include_quoted_loads_file() {
        let dir = unique_dir("include-quoted");
        let header = dir.join("h.h");
        std::fs::write(&header, "int y = 1;\n").unwrap();
        let main = dir.join("main.c");
        std::fs::write(&main, "#include \"h.h\"\nint z;\n").unwrap();

        let mut sm = SourceMap::new();
        let main_text = std::fs::read_to_string(&main).unwrap();
        let main_id = sm.add_file(main.clone(), main_text);
        let (toks, diags) = preprocess(&mut sm, main_id, &PpOptions::default());
        assert!(diags.is_empty(), "{diags:?}");
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        assert_eq!(texts, vec!["int", "y", "=", "1", ";", "int", "z", ";"]);
    }

    #[test]
    fn include_system_uses_include_paths() {
        let dir = unique_dir("include-system");
        let header = dir.join("sys.h");
        std::fs::write(&header, "int s;\n").unwrap();

        let mut sm = SourceMap::new();
        let main_id = sm.add_file("main.c", "#include <sys.h>\n");
        let opts = PpOptions {
            include_paths: vec![dir.clone()],
            user_defines: Vec::new(),
        };
        let (toks, diags) = preprocess(&mut sm, main_id, &opts);
        assert!(diags.is_empty(), "{diags:?}");
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        assert_eq!(texts, vec!["int", "s", ";"]);
    }

    #[test]
    fn include_missing_file_is_diagnosed() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("main.c", "#include \"nope.h\"\n");
        let (_, diags) = preprocess(&mut sm, id, &PpOptions::default());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0203");
    }

    #[test]
    fn include_cycle_is_diagnosed() {
        let dir = unique_dir("include-cycle");
        let a = dir.join("a.h");
        let b = dir.join("b.h");
        std::fs::write(&a, "#include \"b.h\"\n").unwrap();
        std::fs::write(&b, "#include \"a.h\"\n").unwrap();

        let mut sm = SourceMap::new();
        let text = std::fs::read_to_string(&a).unwrap();
        let id = sm.add_file(a, text);
        let (_, diags) = preprocess(&mut sm, id, &PpOptions::default());
        assert!(
            diags.iter().any(|d| d.code.unwrap().0 == "E0204"),
            "{diags:?}"
        );
    }

    #[test]
    fn user_define_from_options() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", "int x = N;\n");
        let opts = PpOptions {
            include_paths: vec![],
            user_defines: vec![UserDefine {
                name: "N".to_string(),
                body: "7".to_string(),
            }],
        };
        let (toks, diags) = preprocess(&mut sm, id, &opts);
        assert!(diags.is_empty(), "{diags:?}");
        let texts: Vec<_> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| text_of(&sm, t))
            .collect();
        assert_eq!(texts, vec!["int", "x", "=", "7", ";"]);
    }
}
