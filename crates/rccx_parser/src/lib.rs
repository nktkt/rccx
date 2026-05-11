//! C parser for `rccx` — Phase 3 MVP.
//!
//! Consumes a post-preprocessor token stream and produces an [`ast::Module`].
//!
//! Out of scope for the MVP (planned for Phase 3.x):
//!
//! - `struct` / `union` / `enum` declarations.
//! - `typedef` and typedef-name disambiguation.
//! - Function pointer declarators.
//! - Designated initializers and compound literals.
//! - `switch` / `case` / `default` and `goto` / labels.
//! - Bit-fields.

use rccx_ast as ast;
use rccx_diagnostics::code::DiagnosticCode;
use rccx_diagnostics::{Diagnostic, Label};
use rccx_lexer::{Keyword, Token, TokenKind};
use rccx_source::{SourceMap, Span};

pub const E_UNEXPECTED_TOKEN: DiagnosticCode = DiagnosticCode("E0301");
pub const E_EXPECTED_TOKEN: DiagnosticCode = DiagnosticCode("E0302");
pub const E_EXPECTED_EXPR: DiagnosticCode = DiagnosticCode("E0303");
pub const E_EXPECTED_TYPE: DiagnosticCode = DiagnosticCode("E0304");
pub const E_EXPECTED_IDENT: DiagnosticCode = DiagnosticCode("E0305");
pub const E_INVALID_TYPE_SPEC: DiagnosticCode = DiagnosticCode("E0306");

/// Parse a token stream into a module.
pub fn parse_module(
    tokens: &[Token],
    sources: &SourceMap,
    root_span: Span,
) -> (ast::Module, Vec<Diagnostic>) {
    let mut p = Parser {
        tokens,
        pos: 0,
        sources,
        diagnostics: Vec::new(),
    };
    let items = p.parse_items_until_eof();
    let module = ast::Module {
        items,
        span: root_span,
    };
    (module, p.diagnostics)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    sources: &'a SourceMap,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    // === Cursor primitives ===

    fn peek(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset)
    }

    fn current(&self) -> Option<&Token> {
        self.peek(0)
    }

    fn current_kind(&self) -> Option<TokenKind> {
        self.current().map(|t| t.kind)
    }

    fn is_eof(&self) -> bool {
        matches!(self.current_kind(), None | Some(TokenKind::Eof))
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.current().copied();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.current_kind() == Some(kind) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> Option<Token> {
        if let Some(tok) = self.current().copied() {
            if tok.kind == kind {
                self.pos += 1;
                return Some(tok);
            }
            self.error(
                tok.span,
                E_EXPECTED_TOKEN,
                format!("expected {what}, found `{}`", self.token_display(&tok)),
            );
        } else {
            let span = self.last_span();
            self.error(
                span,
                E_EXPECTED_TOKEN,
                format!("expected {what}, reached end of input"),
            );
        }
        None
    }

    fn text(&self, tok: &Token) -> &str {
        self.sources
            .file(tok.span.file)
            .and_then(|f| f.slice(tok.span))
            .unwrap_or("")
    }

    fn token_display(&self, tok: &Token) -> String {
        match tok.kind {
            TokenKind::Eof => "<eof>".into(),
            _ => self.text(tok).to_string(),
        }
    }

    fn error(&mut self, span: Span, code: DiagnosticCode, msg: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error(code, msg).with_label(Label::primary_unlabeled(span)));
    }

    fn last_span(&self) -> Span {
        if let Some(t) = self.tokens.last() {
            t.span
        } else {
            Span::DUMMY
        }
    }

    fn span_from(&self, start: Span) -> Span {
        match self.tokens.get(self.pos.saturating_sub(1)) {
            Some(t) if t.span.file == start.file => start.join(t.span),
            _ => start,
        }
    }

    // === Synchronization for error recovery ===

    fn sync_after_statement(&mut self) {
        let mut depth = 0i32;
        while let Some(t) = self.current() {
            match t.kind {
                TokenKind::Semicolon if depth == 0 => {
                    self.pos += 1;
                    return;
                }
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                TokenKind::Eof => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    // === Top level ===

    fn parse_items_until_eof(&mut self) -> Vec<ast::Item> {
        let mut items = Vec::new();
        while !self.is_eof() {
            let start = self.current().map(|t| t.span).unwrap_or(Span::DUMMY);
            match self.parse_item() {
                Some(new_items) => items.extend(new_items),
                None => {
                    // Couldn't parse — skip to next semicolon or brace.
                    if self.current().map(|t| t.span == start).unwrap_or(false) {
                        // Force progress so we don't loop forever.
                        self.bump();
                    }
                    self.sync_after_statement();
                }
            }
        }
        items
    }

    fn parse_item(&mut self) -> Option<Vec<ast::Item>> {
        let start = self.current()?.span;
        let storage = self.parse_storage_specs();
        let base_ty = self.parse_type_specifier()?;

        // First declarator.
        let mut decls = Vec::new();
        let (ty, name) = self.parse_declarator(&base_ty)?;

        // Function form?
        if self.current_kind() == Some(TokenKind::LParen) {
            self.bump();
            let (params, is_variadic) = self.parse_param_list();
            self.expect(TokenKind::RParen, "`)` to close parameter list")?;
            let body = if self.current_kind() == Some(TokenKind::LBrace) {
                Some(self.parse_block()?)
            } else {
                self.expect(TokenKind::Semicolon, "`;` after function prototype")?;
                None
            };
            let span = self.span_from(start);
            return Some(vec![ast::Item::FnDef(ast::FnDef {
                return_type: ty,
                name,
                params,
                is_variadic,
                body,
                storage,
                span,
            })]);
        }

        // Variable declaration; optional array suffix and initializer.
        let ty = self.parse_array_suffix(ty);
        let init = if self.eat(TokenKind::Eq) {
            Some(self.parse_assignment_expr()?)
        } else {
            None
        };
        let span = self.span_from(start);
        decls.push(ast::Decl {
            ty,
            name,
            init,
            storage: storage.clone(),
            span,
        });

        while self.eat(TokenKind::Comma) {
            let (mut ty, name) = self.parse_declarator(&base_ty)?;
            ty = self.parse_array_suffix(ty);
            let init = if self.eat(TokenKind::Eq) {
                Some(self.parse_assignment_expr()?)
            } else {
                None
            };
            let span = self.span_from(start);
            decls.push(ast::Decl {
                ty,
                name,
                init,
                storage: storage.clone(),
                span,
            });
        }

        self.expect(TokenKind::Semicolon, "`;` after declaration")?;
        Some(decls.into_iter().map(ast::Item::Decl).collect())
    }

    // === Type & declarators ===

    fn parse_storage_specs(&mut self) -> Vec<ast::StorageSpec> {
        let mut out = Vec::new();
        while let Some(tok) = self.current() {
            let spec = match tok.kind {
                TokenKind::Keyword(Keyword::Static) => ast::StorageSpec::Static,
                TokenKind::Keyword(Keyword::Extern) => ast::StorageSpec::Extern,
                TokenKind::Keyword(Keyword::Auto) => ast::StorageSpec::Auto,
                TokenKind::Keyword(Keyword::Register) => ast::StorageSpec::Register,
                TokenKind::Keyword(Keyword::Inline) => ast::StorageSpec::Inline,
                TokenKind::Keyword(Keyword::Noreturn_) => ast::StorageSpec::Noreturn,
                TokenKind::Keyword(Keyword::ThreadLocal_)
                | TokenKind::Keyword(Keyword::ThreadLocal) => ast::StorageSpec::ThreadLocal,
                _ => break,
            };
            out.push(spec);
            self.bump();
        }
        out
    }

    fn parse_qualifiers(&mut self) -> ast::TypeQualifiers {
        let mut q = ast::TypeQualifiers::default();
        while let Some(tok) = self.current() {
            match tok.kind {
                TokenKind::Keyword(Keyword::Const) => q.is_const = true,
                TokenKind::Keyword(Keyword::Volatile) => q.is_volatile = true,
                TokenKind::Keyword(Keyword::Restrict) => q.is_restrict = true,
                TokenKind::Keyword(Keyword::Atomic_) => q.is_atomic = true,
                _ => break,
            }
            self.bump();
        }
        q
    }

    fn parse_type_specifier(&mut self) -> Option<ast::Type> {
        let start = self.current()?.span;
        let mut qualifiers = self.parse_qualifiers();

        // Collect type-specifier keywords.
        let mut signed = 0; // -1 = signed, +1 = unsigned, 0 = unspecified
        let mut shorts = 0u32;
        let mut longs = 0u32;
        let mut base: Option<BaseSpec> = None;

        loop {
            // Allow qualifiers to be interleaved with type specifiers.
            let pre = self.parse_qualifiers();
            qualifiers = merge_qualifiers(qualifiers, pre);

            let Some(tok) = self.current() else { break };
            match tok.kind {
                TokenKind::Keyword(Keyword::Void) => {
                    if base.is_some() {
                        self.error(
                            tok.span,
                            E_INVALID_TYPE_SPEC,
                            "`void` cannot be combined with other type specifiers",
                        );
                    }
                    base = Some(BaseSpec::Void);
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Bool_) | TokenKind::Keyword(Keyword::Bool) => {
                    base = Some(BaseSpec::Bool);
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Char) => {
                    base = Some(BaseSpec::Char);
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Short) => {
                    shorts += 1;
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Int) => {
                    base = Some(BaseSpec::Int);
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Long) => {
                    longs += 1;
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Float) => {
                    base = Some(BaseSpec::Float);
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Double) => {
                    base = Some(BaseSpec::Double);
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Signed) => {
                    signed = -1;
                    self.bump();
                }
                TokenKind::Keyword(Keyword::Unsigned) => {
                    signed = 1;
                    self.bump();
                }
                _ => break,
            }
        }

        let kind = match (base, shorts, longs, signed) {
            (Some(BaseSpec::Void), 0, 0, 0) => ast::TypeKind::Void,
            (Some(BaseSpec::Bool), 0, 0, 0) => ast::TypeKind::Bool,
            (Some(BaseSpec::Float), 0, 0, 0) => ast::TypeKind::Builtin(ast::BuiltinType::Float),
            (Some(BaseSpec::Double), 0, 0, 0) => ast::TypeKind::Builtin(ast::BuiltinType::Double),
            (Some(BaseSpec::Double), 0, 1, 0) => {
                ast::TypeKind::Builtin(ast::BuiltinType::LongDouble)
            }
            (Some(BaseSpec::Char), 0, 0, 0) => ast::TypeKind::Builtin(ast::BuiltinType::Char),
            (Some(BaseSpec::Char), 0, 0, -1) => ast::TypeKind::Builtin(ast::BuiltinType::SChar),
            (Some(BaseSpec::Char), 0, 0, 1) => ast::TypeKind::Builtin(ast::BuiltinType::UChar),
            (_, 1, 0, s) => ast::TypeKind::Builtin(if s == 1 {
                ast::BuiltinType::UShort
            } else {
                ast::BuiltinType::Short
            }),
            (_, 0, 0, s) => ast::TypeKind::Builtin(if s == 1 {
                ast::BuiltinType::UInt
            } else {
                ast::BuiltinType::Int
            }),
            (_, 0, 1, s) => ast::TypeKind::Builtin(if s == 1 {
                ast::BuiltinType::ULong
            } else {
                ast::BuiltinType::Long
            }),
            (_, 0, 2, s) => ast::TypeKind::Builtin(if s == 1 {
                ast::BuiltinType::ULongLong
            } else {
                ast::BuiltinType::LongLong
            }),
            (_, _, _, _) => {
                let span = self.current().map(|t| t.span).unwrap_or(start);
                self.error(span, E_EXPECTED_TYPE, "expected a type specifier");
                return None;
            }
        };

        let span = self.span_from(start);
        let mut ty = ast::Type {
            kind,
            qualifiers,
            span,
        };
        let post = self.parse_qualifiers();
        ty.qualifiers = merge_qualifiers(ty.qualifiers, post);
        Some(ty)
    }

    /// Parse pointer levels and the identifier of a declarator. Returns
    /// the rewritten type and the name.
    fn parse_declarator(&mut self, base: &ast::Type) -> Option<(ast::Type, ast::Ident)> {
        let mut ty = base.clone();
        let start = self.current()?.span;
        while self.eat(TokenKind::Star) {
            let quals = self.parse_qualifiers();
            ty = ast::Type {
                kind: ast::TypeKind::Pointer(Box::new(ty)),
                qualifiers: quals,
                span: self.span_from(start),
            };
        }
        let name = self.parse_ident()?;
        Some((ty, name))
    }

    fn parse_array_suffix(&mut self, mut ty: ast::Type) -> ast::Type {
        while self.current_kind() == Some(TokenKind::LBracket) {
            let start = self.current().unwrap().span;
            self.bump();
            let size = if self.current_kind() == Some(TokenKind::RBracket) {
                None
            } else {
                let e = self.parse_assignment_expr();
                e.map(Box::new)
            };
            let _ = self.expect(TokenKind::RBracket, "`]` to close array bound");
            ty = ast::Type {
                kind: ast::TypeKind::Array {
                    elem: Box::new(ty),
                    size,
                },
                qualifiers: ast::TypeQualifiers::default(),
                span: self.span_from(start),
            };
        }
        ty
    }

    fn parse_param_list(&mut self) -> (Vec<ast::Param>, bool) {
        let mut params = Vec::new();
        let mut is_variadic = false;

        // Empty `()` -> zero parameters (we treat it like a prototype).
        if self.current_kind() == Some(TokenKind::RParen) {
            return (params, false);
        }

        // `(void)` -> zero parameters.
        if self.current_kind() == Some(TokenKind::Keyword(Keyword::Void))
            && self.peek(1).map(|t| t.kind) == Some(TokenKind::RParen)
        {
            self.bump();
            return (params, false);
        }

        loop {
            if self.current_kind() == Some(TokenKind::Ellipsis) {
                self.bump();
                is_variadic = true;
                break;
            }
            let start = match self.current() {
                Some(t) => t.span,
                None => break,
            };
            let Some(ty) = self.parse_type_specifier() else {
                break;
            };
            // Pointer levels.
            let mut ty = ty;
            while self.eat(TokenKind::Star) {
                let quals = self.parse_qualifiers();
                ty = ast::Type {
                    kind: ast::TypeKind::Pointer(Box::new(ty)),
                    qualifiers: quals,
                    span: self.span_from(start),
                };
            }
            // Optional identifier for the parameter.
            let name = if self.current_kind() == Some(TokenKind::Ident) {
                self.parse_ident()
            } else {
                None
            };
            let ty = self.parse_array_suffix(ty);
            let span = self.span_from(start);
            params.push(ast::Param { ty, name, span });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        (params, is_variadic)
    }

    fn parse_ident(&mut self) -> Option<ast::Ident> {
        let tok = self.current().copied();
        match tok {
            Some(t) if t.kind == TokenKind::Ident => {
                self.bump();
                Some(ast::Ident {
                    name: self.text(&t).to_string(),
                    span: t.span,
                })
            }
            Some(t) => {
                self.error(
                    t.span,
                    E_EXPECTED_IDENT,
                    format!("expected identifier, found `{}`", self.token_display(&t)),
                );
                None
            }
            None => {
                let span = self.last_span();
                self.error(
                    span,
                    E_EXPECTED_IDENT,
                    "expected identifier, reached end of input",
                );
                None
            }
        }
    }

    // === Statements ===

    fn parse_block(&mut self) -> Option<ast::Block> {
        let lbrace = self.expect(TokenKind::LBrace, "`{` to begin a block")?;
        let mut stmts = Vec::new();
        while !self.is_eof() && self.current_kind() != Some(TokenKind::RBrace) {
            let pre_pos = self.pos;
            match self.parse_stmt() {
                Some(s) => stmts.push(s),
                None => {
                    if self.pos == pre_pos {
                        self.bump();
                    }
                    self.sync_after_statement();
                }
            }
        }
        let _ = self.expect(TokenKind::RBrace, "`}` to close block");
        let span = self.span_from(lbrace.span);
        Some(ast::Block { stmts, span })
    }

    fn parse_stmt(&mut self) -> Option<ast::Stmt> {
        let start = self.current()?.span;
        match self.current_kind()? {
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Some(ast::Stmt {
                    kind: ast::StmtKind::Compound(block),
                    span: self.span_from(start),
                })
            }
            TokenKind::Semicolon => {
                self.bump();
                Some(ast::Stmt {
                    kind: ast::StmtKind::Empty,
                    span: self.span_from(start),
                })
            }
            TokenKind::Keyword(Keyword::Return) => {
                self.bump();
                let value = if self.current_kind() == Some(TokenKind::Semicolon) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                self.expect(TokenKind::Semicolon, "`;` after return statement")?;
                Some(ast::Stmt {
                    kind: ast::StmtKind::Return(value),
                    span: self.span_from(start),
                })
            }
            TokenKind::Keyword(Keyword::Break) => {
                self.bump();
                self.expect(TokenKind::Semicolon, "`;` after `break`")?;
                Some(ast::Stmt {
                    kind: ast::StmtKind::Break,
                    span: self.span_from(start),
                })
            }
            TokenKind::Keyword(Keyword::Continue) => {
                self.bump();
                self.expect(TokenKind::Semicolon, "`;` after `continue`")?;
                Some(ast::Stmt {
                    kind: ast::StmtKind::Continue,
                    span: self.span_from(start),
                })
            }
            TokenKind::Keyword(Keyword::If) => self.parse_if(start),
            TokenKind::Keyword(Keyword::While) => self.parse_while(start),
            TokenKind::Keyword(Keyword::Do) => self.parse_do_while(start),
            TokenKind::Keyword(Keyword::For) => self.parse_for(start),
            _ if self.looks_like_decl_start() => {
                let decls = self.parse_local_decl_stmt()?;
                Some(ast::Stmt {
                    kind: ast::StmtKind::Decl(decls),
                    span: self.span_from(start),
                })
            }
            _ => {
                let expr = self.parse_expr()?;
                self.expect(TokenKind::Semicolon, "`;` after expression statement")?;
                Some(ast::Stmt {
                    kind: ast::StmtKind::Expr(expr),
                    span: self.span_from(start),
                })
            }
        }
    }

    fn parse_if(&mut self, start: Span) -> Option<ast::Stmt> {
        self.bump(); // `if`
        self.expect(TokenKind::LParen, "`(` after `if`")?;
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, "`)` to close `if` condition")?;
        let then_branch = Box::new(self.parse_stmt()?);
        let else_branch = if self.eat(TokenKind::Keyword(Keyword::Else)) {
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };
        Some(ast::Stmt {
            kind: ast::StmtKind::If {
                cond,
                then_branch,
                else_branch,
            },
            span: self.span_from(start),
        })
    }

    fn parse_while(&mut self, start: Span) -> Option<ast::Stmt> {
        self.bump();
        self.expect(TokenKind::LParen, "`(` after `while`")?;
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, "`)` to close `while` condition")?;
        let body = Box::new(self.parse_stmt()?);
        Some(ast::Stmt {
            kind: ast::StmtKind::While { cond, body },
            span: self.span_from(start),
        })
    }

    fn parse_do_while(&mut self, start: Span) -> Option<ast::Stmt> {
        self.bump();
        let body = Box::new(self.parse_stmt()?);
        self.expect(
            TokenKind::Keyword(Keyword::While),
            "`while` after `do` body",
        )?;
        self.expect(TokenKind::LParen, "`(` after `while`")?;
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, "`)` to close `while` condition")?;
        self.expect(TokenKind::Semicolon, "`;` after `do-while`")?;
        Some(ast::Stmt {
            kind: ast::StmtKind::DoWhile { body, cond },
            span: self.span_from(start),
        })
    }

    fn parse_for(&mut self, start: Span) -> Option<ast::Stmt> {
        self.bump();
        self.expect(TokenKind::LParen, "`(` after `for`")?;
        let init = if self.eat(TokenKind::Semicolon) {
            None
        } else if self.looks_like_decl_start() {
            let decls = self.parse_local_decl_stmt()?;
            Some(ast::ForInit::Decl(decls))
        } else {
            let e = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, "`;` after for-init")?;
            Some(ast::ForInit::Expr(e))
        };
        let cond = if self.current_kind() == Some(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semicolon, "`;` after for-condition")?;
        let step = if self.current_kind() == Some(TokenKind::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::RParen, "`)` to close `for` header")?;
        let body = Box::new(self.parse_stmt()?);
        Some(ast::Stmt {
            kind: ast::StmtKind::For {
                init,
                cond,
                step,
                body,
            },
            span: self.span_from(start),
        })
    }

    fn parse_local_decl_stmt(&mut self) -> Option<Vec<ast::Decl>> {
        let storage = self.parse_storage_specs();
        let base = self.parse_type_specifier()?;
        let mut decls = Vec::new();
        loop {
            let (mut ty, name) = self.parse_declarator(&base)?;
            ty = self.parse_array_suffix(ty);
            let init = if self.eat(TokenKind::Eq) {
                Some(self.parse_assignment_expr()?)
            } else {
                None
            };
            let span = self.span_from(name.span);
            decls.push(ast::Decl {
                ty,
                name,
                init,
                storage: storage.clone(),
                span,
            });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::Semicolon, "`;` after declaration")?;
        Some(decls)
    }

    fn looks_like_decl_start(&self) -> bool {
        let Some(tok) = self.current() else {
            return false;
        };
        matches!(
            tok.kind,
            TokenKind::Keyword(
                Keyword::Void
                    | Keyword::Bool
                    | Keyword::Bool_
                    | Keyword::Char
                    | Keyword::Short
                    | Keyword::Int
                    | Keyword::Long
                    | Keyword::Float
                    | Keyword::Double
                    | Keyword::Signed
                    | Keyword::Unsigned
                    | Keyword::Const
                    | Keyword::Volatile
                    | Keyword::Restrict
                    | Keyword::Atomic_
                    | Keyword::Static
                    | Keyword::Extern
                    | Keyword::Auto
                    | Keyword::Register
                    | Keyword::Inline
                    | Keyword::Noreturn_
                    | Keyword::ThreadLocal_
                    | Keyword::ThreadLocal
            )
        )
    }

    // === Expressions (Pratt) ===

    fn parse_expr(&mut self) -> Option<ast::Expr> {
        // Top-level expr handles the comma operator.
        let first = self.parse_assignment_expr()?;
        if self.current_kind() != Some(TokenKind::Comma) {
            return Some(first);
        }
        let start = first.span;
        let mut parts = vec![first];
        while self.eat(TokenKind::Comma) {
            parts.push(self.parse_assignment_expr()?);
        }
        Some(ast::Expr {
            kind: ast::ExprKind::Comma(parts),
            span: self.span_from(start),
        })
    }

    fn parse_assignment_expr(&mut self) -> Option<ast::Expr> {
        let lhs = self.parse_ternary_expr()?;
        let op = match self.current_kind()? {
            TokenKind::Eq => Some(ast::AssignOp::Assign),
            TokenKind::PlusEq => Some(ast::AssignOp::AddAssign),
            TokenKind::MinusEq => Some(ast::AssignOp::SubAssign),
            TokenKind::StarEq => Some(ast::AssignOp::MulAssign),
            TokenKind::SlashEq => Some(ast::AssignOp::DivAssign),
            TokenKind::PercentEq => Some(ast::AssignOp::ModAssign),
            TokenKind::ShlEq => Some(ast::AssignOp::ShlAssign),
            TokenKind::ShrEq => Some(ast::AssignOp::ShrAssign),
            TokenKind::AmpEq => Some(ast::AssignOp::AndAssign),
            TokenKind::CaretEq => Some(ast::AssignOp::XorAssign),
            TokenKind::PipeEq => Some(ast::AssignOp::OrAssign),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let rhs = self.parse_assignment_expr()?;
            let span = lhs.span.join(rhs.span);
            return Some(ast::Expr {
                kind: ast::ExprKind::Assign {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            });
        }
        Some(lhs)
    }

    fn parse_ternary_expr(&mut self) -> Option<ast::Expr> {
        let cond = self.parse_binary_expr(0)?;
        if !self.eat(TokenKind::Question) {
            return Some(cond);
        }
        let then_branch = self.parse_expr()?;
        self.expect(TokenKind::Colon, "`:` in ternary expression")?;
        let else_branch = self.parse_assignment_expr()?;
        let span = cond.span.join(else_branch.span);
        Some(ast::Expr {
            kind: ast::ExprKind::Ternary {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: Box::new(else_branch),
            },
            span,
        })
    }

    fn parse_binary_expr(&mut self, min_prec: u8) -> Option<ast::Expr> {
        let mut lhs = self.parse_unary_expr()?;
        while let Some((op, prec)) = self.current_kind().and_then(binop_for) {
            if prec < min_prec {
                break;
            }
            self.bump();
            let rhs = self.parse_binary_expr(prec + 1)?;
            let span = lhs.span.join(rhs.span);
            lhs = ast::Expr {
                kind: ast::ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Some(lhs)
    }

    fn parse_unary_expr(&mut self) -> Option<ast::Expr> {
        let tok = self.current().copied()?;
        let op = match tok.kind {
            TokenKind::Plus => Some(ast::UnaryOp::Plus),
            TokenKind::Minus => Some(ast::UnaryOp::Neg),
            TokenKind::Bang => Some(ast::UnaryOp::LogicalNot),
            TokenKind::Tilde => Some(ast::UnaryOp::BitNot),
            TokenKind::Star => Some(ast::UnaryOp::Deref),
            TokenKind::Amp => Some(ast::UnaryOp::AddrOf),
            TokenKind::PlusPlus => Some(ast::UnaryOp::PreInc),
            TokenKind::MinusMinus => Some(ast::UnaryOp::PreDec),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let operand = self.parse_unary_expr()?;
            let span = tok.span.join(operand.span);
            return Some(ast::Expr {
                kind: ast::ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
                span,
            });
        }
        if tok.kind == TokenKind::Keyword(Keyword::Sizeof) {
            self.bump();
            let operand = self.parse_unary_expr()?;
            let span = tok.span.join(operand.span);
            return Some(ast::Expr {
                kind: ast::ExprKind::SizeofExpr(Box::new(operand)),
                span,
            });
        }
        self.parse_postfix_expr()
    }

    fn parse_postfix_expr(&mut self) -> Option<ast::Expr> {
        let mut expr = self.parse_primary_expr()?;
        loop {
            match self.current_kind() {
                Some(TokenKind::LParen) => {
                    self.bump();
                    let mut args = Vec::new();
                    if self.current_kind() != Some(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_assignment_expr()?);
                            if !self.eat(TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    let close = self.expect(TokenKind::RParen, "`)` to close call")?;
                    let span = expr.span.join(close.span);
                    expr = ast::Expr {
                        kind: ast::ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                Some(TokenKind::LBracket) => {
                    self.bump();
                    let index = self.parse_expr()?;
                    let close = self.expect(TokenKind::RBracket, "`]` to close index")?;
                    let span = expr.span.join(close.span);
                    expr = ast::Expr {
                        kind: ast::ExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    };
                }
                Some(TokenKind::Dot) | Some(TokenKind::Arrow) => {
                    let arrow = self.current_kind() == Some(TokenKind::Arrow);
                    self.bump();
                    let field = self.parse_ident()?;
                    let span = expr.span.join(field.span);
                    expr = ast::Expr {
                        kind: ast::ExprKind::Member {
                            base: Box::new(expr),
                            arrow,
                            field,
                        },
                        span,
                    };
                }
                Some(TokenKind::PlusPlus) => {
                    let tok = self.bump().unwrap();
                    let span = expr.span.join(tok.span);
                    expr = ast::Expr {
                        kind: ast::ExprKind::Postfix {
                            op: ast::PostfixOp::Inc,
                            operand: Box::new(expr),
                        },
                        span,
                    };
                }
                Some(TokenKind::MinusMinus) => {
                    let tok = self.bump().unwrap();
                    let span = expr.span.join(tok.span);
                    expr = ast::Expr {
                        kind: ast::ExprKind::Postfix {
                            op: ast::PostfixOp::Dec,
                            operand: Box::new(expr),
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Some(expr)
    }

    fn parse_primary_expr(&mut self) -> Option<ast::Expr> {
        let tok = self.current().copied()?;
        match tok.kind {
            TokenKind::Ident => {
                self.bump();
                Some(ast::Expr {
                    kind: ast::ExprKind::Ident(ast::Ident {
                        name: self.text(&tok).to_string(),
                        span: tok.span,
                    }),
                    span: tok.span,
                })
            }
            TokenKind::IntLiteral { .. } => {
                self.bump();
                Some(ast::Expr {
                    kind: ast::ExprKind::IntLit(ast::IntLit {
                        raw: self.text(&tok).to_string(),
                    }),
                    span: tok.span,
                })
            }
            TokenKind::FloatLiteral => {
                self.bump();
                Some(ast::Expr {
                    kind: ast::ExprKind::FloatLit(ast::FloatLit {
                        raw: self.text(&tok).to_string(),
                    }),
                    span: tok.span,
                })
            }
            TokenKind::StringLiteral => {
                self.bump();
                Some(ast::Expr {
                    kind: ast::ExprKind::StringLit(self.text(&tok).to_string()),
                    span: tok.span,
                })
            }
            TokenKind::CharLiteral => {
                self.bump();
                Some(ast::Expr {
                    kind: ast::ExprKind::CharLit(self.text(&tok).to_string()),
                    span: tok.span,
                })
            }
            TokenKind::LParen => {
                self.bump();
                // Cast vs parenthesized? If the next token starts a type, treat
                // as a cast.
                if self.looks_like_decl_start() {
                    let ty = self.parse_type_specifier()?;
                    // Optional pointer levels in cast.
                    let mut ty = ty;
                    let cast_start = tok.span;
                    while self.eat(TokenKind::Star) {
                        let quals = self.parse_qualifiers();
                        ty = ast::Type {
                            kind: ast::TypeKind::Pointer(Box::new(ty)),
                            qualifiers: quals,
                            span: self.span_from(cast_start),
                        };
                    }
                    self.expect(TokenKind::RParen, "`)` to close cast type")?;
                    let inner = self.parse_unary_expr()?;
                    let span = tok.span.join(inner.span);
                    Some(ast::Expr {
                        kind: ast::ExprKind::Cast {
                            ty,
                            expr: Box::new(inner),
                        },
                        span,
                    })
                } else {
                    let inner = self.parse_expr()?;
                    let close =
                        self.expect(TokenKind::RParen, "`)` to close parenthesized expression")?;
                    let span = tok.span.join(close.span);
                    Some(ast::Expr {
                        kind: ast::ExprKind::Paren(Box::new(inner)),
                        span,
                    })
                }
            }
            _ => {
                self.error(
                    tok.span,
                    E_EXPECTED_EXPR,
                    format!("expected expression, found `{}`", self.token_display(&tok)),
                );
                None
            }
        }
    }
}

fn merge_qualifiers(a: ast::TypeQualifiers, b: ast::TypeQualifiers) -> ast::TypeQualifiers {
    ast::TypeQualifiers {
        is_const: a.is_const || b.is_const,
        is_volatile: a.is_volatile || b.is_volatile,
        is_restrict: a.is_restrict || b.is_restrict,
        is_atomic: a.is_atomic || b.is_atomic,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaseSpec {
    Void,
    Bool,
    Char,
    Int,
    Float,
    Double,
}

fn binop_for(kind: TokenKind) -> Option<(ast::BinaryOp, u8)> {
    use ast::BinaryOp as B;
    let (op, prec) = match kind {
        TokenKind::PipePipe => (B::LogicalOr, 1),
        TokenKind::AmpAmp => (B::LogicalAnd, 2),
        TokenKind::Pipe => (B::BitOr, 3),
        TokenKind::Caret => (B::BitXor, 4),
        TokenKind::Amp => (B::BitAnd, 5),
        TokenKind::EqEq => (B::Eq, 6),
        TokenKind::BangEq => (B::NotEq, 6),
        TokenKind::Lt => (B::Lt, 7),
        TokenKind::Gt => (B::Gt, 7),
        TokenKind::LtEq => (B::LtEq, 7),
        TokenKind::GtEq => (B::GtEq, 7),
        TokenKind::Shl => (B::Shl, 8),
        TokenKind::Shr => (B::Shr, 8),
        TokenKind::Plus => (B::Add, 9),
        TokenKind::Minus => (B::Sub, 9),
        TokenKind::Star => (B::Mul, 10),
        TokenKind::Slash => (B::Div, 10),
        TokenKind::Percent => (B::Mod, 10),
        _ => return None,
    };
    Some((op, prec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rccx_pp::{preprocess, PpOptions};
    use rccx_source::SourceMap;

    fn parse_str(src: &str) -> (ast::Module, Vec<Diagnostic>, SourceMap) {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", src);
        let (tokens, mut diags) = preprocess(&mut sm, id, &PpOptions::default());
        let root_span = sm
            .file(id)
            .map(|f| Span::new(id, 0, f.text().len() as u32))
            .unwrap();
        let (module, pdiags) = parse_module(&tokens, &sm, root_span);
        diags.extend(pdiags);
        (module, diags, sm)
    }

    fn dump_clean(src: &str) -> String {
        let (m, diags, _) = parse_str(src);
        assert!(diags.is_empty(), "diagnostics: {diags:?}");
        ast::dump(&m)
    }

    #[test]
    fn parses_empty_module() {
        let (m, diags, _) = parse_str("");
        assert!(diags.is_empty());
        assert!(m.items.is_empty());
    }

    #[test]
    fn parses_global_variable() {
        let s = dump_clean("int x = 42;\n");
        assert!(s.contains("Decl `x` : int"), "{s}");
        assert!(s.contains("IntLit 42"), "{s}");
    }

    #[test]
    fn parses_pointer_variable() {
        let s = dump_clean("int *p;\n");
        assert!(s.contains("Decl `p` : int*"), "{s}");
    }

    #[test]
    fn parses_array_variable() {
        let s = dump_clean("int arr[10];\n");
        assert!(s.contains("Decl `arr` : int[N]"), "{s}");
    }

    #[test]
    fn parses_multi_declarator() {
        let s = dump_clean("int a, b = 2;\n");
        assert!(s.matches("Decl `").count() >= 2, "{s}");
        assert!(s.contains("Decl `a`"), "{s}");
        assert!(s.contains("Decl `b`"), "{s}");
    }

    #[test]
    fn parses_hello_world_function() {
        let s = dump_clean("int main(void) {\n    return 0;\n}\n");
        assert!(s.contains("FnDef `main` -> int"), "{s}");
        assert!(s.contains("Return"), "{s}");
        assert!(s.contains("IntLit 0"), "{s}");
    }

    #[test]
    fn parses_function_with_params() {
        let s = dump_clean("int add(int a, int b) { return a + b; }\n");
        assert!(s.contains("Param `a` : int"), "{s}");
        assert!(s.contains("Param `b` : int"), "{s}");
        assert!(s.contains("Binary Add"), "{s}");
    }

    #[test]
    fn parses_prototype() {
        let s = dump_clean("int foo(int);\n");
        assert!(s.contains("FnDecl `foo`"), "{s}");
        assert!(!s.contains("body:"), "{s}");
    }

    #[test]
    fn parses_variadic_prototype() {
        let s = dump_clean("int printf(const char *fmt, ...);\n");
        assert!(s.contains("FnDecl `printf`"), "{s}");
        assert!(s.contains("..."), "{s}");
    }

    #[test]
    fn parses_if_else() {
        let s = dump_clean("int main(void) { if (1) return 2; else return 3; }\n");
        assert!(s.contains("If"), "{s}");
        assert!(s.contains("then:"), "{s}");
        assert!(s.contains("else:"), "{s}");
    }

    #[test]
    fn parses_while_and_break() {
        let s = dump_clean("int main(void) { while (1) { break; } }\n");
        assert!(s.contains("While"), "{s}");
        assert!(s.contains("Break"), "{s}");
    }

    #[test]
    fn parses_do_while() {
        let s = dump_clean("int main(void) { do { } while (0); }\n");
        assert!(s.contains("DoWhile"), "{s}");
    }

    #[test]
    fn parses_for_with_local_decl() {
        let s = dump_clean("int main(void) { for (int i = 0; i < 10; i = i + 1) { } return 0; }\n");
        assert!(s.contains("For"), "{s}");
        assert!(s.contains("Decl `i`"), "{s}");
    }

    #[test]
    fn parses_call_and_member() {
        let s = dump_clean("int main(void) { foo(1, 2); p->x; obj.y; return 0; }\n");
        assert!(s.contains("Call"), "{s}");
        assert!(s.contains("Member ->x"), "{s}");
        assert!(s.contains("Member .y"), "{s}");
    }

    #[test]
    fn precedence_multiplication_binds_tighter_than_addition() {
        let s = dump_clean("int main(void) { return 1 + 2 * 3; }\n");
        // Outermost binary should be Add; nested Mul on the right.
        let i_add = s.find("Binary Add").unwrap();
        let i_mul = s.find("Binary Mul").unwrap();
        assert!(i_add < i_mul, "{s}");
    }

    #[test]
    fn ternary_and_assignment_compose() {
        let s = dump_clean("int main(void) { int x; x = (1 ? 2 : 3); return x; }\n");
        assert!(s.contains("Assign Assign"), "{s}");
        assert!(s.contains("Ternary"), "{s}");
    }

    #[test]
    fn cast_in_expression() {
        let s = dump_clean("int main(void) { return (int)0; }\n");
        assert!(s.contains("Cast to int"), "{s}");
    }

    #[test]
    fn unexpected_token_recovers() {
        let (_, diags, _) = parse_str("int x = ;\nint y;\n");
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0303"));
    }

    #[test]
    fn missing_semicolon_diagnoses() {
        let (_, diags, _) = parse_str("int x = 1\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0302"));
    }

    #[test]
    fn unsigned_long_long_type() {
        let s = dump_clean("unsigned long long x;\n");
        assert!(s.contains("unsigned long long"), "{s}");
    }

    #[test]
    fn const_pointer_type() {
        let s = dump_clean("const int *p;\n");
        assert!(s.contains("const int*"), "{s}");
    }
}
