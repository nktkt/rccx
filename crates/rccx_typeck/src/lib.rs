//! Name resolution + type checker for `rccx` — Phase 4 MVP.
//!
//! Two-pass strategy:
//!
//! 1. Walk the module top-level, declaring every function and global symbol
//!    so bodies can refer to symbols that appear later in the file.
//! 2. Walk every function body, resolving identifiers and computing types
//!    for expressions, inserting `ImplicitCast` nodes where needed.
//!
//! Diagnostics: `E0401..E0406` are owned by this stage.

use std::collections::HashMap;

use rccx_ast as ast;
use rccx_diagnostics::code::DiagnosticCode;
use rccx_diagnostics::{Diagnostic, Label};
use rccx_hir::{self as hir, HirType, Ownership, SymbolId};
use rccx_source::Span;

pub const E_UNDEFINED_IDENT: DiagnosticCode = DiagnosticCode("E0401");
pub const E_REDEFINITION: DiagnosticCode = DiagnosticCode("E0402");
pub const E_TYPE_MISMATCH: DiagnosticCode = DiagnosticCode("E0403");
pub const E_ARITY_MISMATCH: DiagnosticCode = DiagnosticCode("E0404");
pub const E_INVALID_OPERAND: DiagnosticCode = DiagnosticCode("E0405");
pub const E_INVALID_LVALUE: DiagnosticCode = DiagnosticCode("E0406");

/// Run name resolution + type checking on `module`. Returns the lowered HIR
/// and any diagnostics produced.
pub fn check(module: &ast::Module) -> (hir::HirModule, Vec<Diagnostic>) {
    let mut tc = TypeChecker::new();
    tc.declare_top_level(module);
    let items = tc.lower_items(module);
    let hir_module = hir::HirModule {
        items,
        symbols: tc.symbols,
        span: module.span,
    };
    (hir_module, tc.diagnostics)
}

struct TypeChecker {
    symbols: hir::SymbolTable,
    /// Stack of lexical scopes. The bottom entry is the global scope.
    scopes: Vec<Scope>,
    diagnostics: Vec<Diagnostic>,
    current_return_type: Option<HirType>,
    loop_depth: u32,
}

#[derive(Default)]
struct Scope {
    names: HashMap<String, SymbolId>,
}

impl TypeChecker {
    fn new() -> Self {
        Self {
            symbols: hir::SymbolTable::new(),
            scopes: vec![Scope::default()],
            diagnostics: Vec::new(),
            current_return_type: None,
            loop_depth: 0,
        }
    }

    // === Diagnostics ===

    fn error(&mut self, span: Span, code: DiagnosticCode, msg: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error(code, msg).with_label(Label::primary_unlabeled(span)));
    }

    // === Scope handling ===

    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, sym: SymbolId, span: Span) -> bool {
        let scope = self.scopes.last_mut().expect("at least one scope");
        if scope.names.contains_key(name) {
            self.error(
                span,
                E_REDEFINITION,
                format!("`{name}` is already defined in this scope"),
            );
            return false;
        }
        scope.names.insert(name.to_string(), sym);
        true
    }

    fn lookup(&self, name: &str) -> Option<SymbolId> {
        for scope in self.scopes.iter().rev() {
            if let Some(id) = scope.names.get(name) {
                return Some(*id);
            }
        }
        None
    }

    // === Top-level pass ===

    fn declare_top_level(&mut self, module: &ast::Module) {
        for item in &module.items {
            match item {
                ast::Item::FnDef(f) => self.declare_fn(f),
                ast::Item::Decl(d) => self.declare_global(d),
            }
        }
    }

    fn declare_fn(&mut self, f: &ast::FnDef) {
        let ret_ty = self.lower_type(&f.return_type);
        let mut param_tys = Vec::with_capacity(f.params.len());
        for p in &f.params {
            param_tys.push(self.lower_type(&p.ty));
        }
        let fn_ty = HirType::Function {
            ret: Box::new(ret_ty),
            params: param_tys,
            is_variadic: f.is_variadic,
        };
        // If a prototype already exists, do not redefine; just keep the first.
        if let Some(existing) = self.lookup(&f.name.name) {
            // Allow a definition to follow a prototype, but flag conflicting
            // re-declarations of the same name with a different shape.
            let cur = self.symbols.get(existing).ty.clone();
            if cur != fn_ty {
                self.error(
                    f.span,
                    E_REDEFINITION,
                    format!("redeclaration of `{}` with a different type", f.name.name),
                );
            }
            return;
        }
        let sym = self.symbols.intern(hir::Symbol {
            name: f.name.name.clone(),
            kind: hir::SymbolKind::Function,
            ty: fn_ty,
            span: f.name.span,
        });
        self.declare(&f.name.name, sym, f.name.span);
    }

    fn declare_global(&mut self, d: &ast::Decl) {
        let ty = self.lower_type(&d.ty);
        let sym = self.symbols.intern(hir::Symbol {
            name: d.name.name.clone(),
            kind: hir::SymbolKind::GlobalVar,
            ty,
            span: d.name.span,
        });
        self.declare(&d.name.name, sym, d.name.span);
    }

    // === Body lowering ===

    fn lower_items(&mut self, module: &ast::Module) -> Vec<hir::HirItem> {
        let mut items = Vec::new();
        for item in &module.items {
            match item {
                ast::Item::FnDef(f) => {
                    if let Some(hf) = self.lower_fn(f) {
                        items.push(hir::HirItem::FnDef(hf));
                    }
                }
                ast::Item::Decl(d) => {
                    if let Some(hd) = self.lower_global(d) {
                        items.push(hir::HirItem::Decl(hd));
                    }
                }
            }
        }
        items
    }

    fn lower_fn(&mut self, f: &ast::FnDef) -> Option<hir::HirFnDef> {
        let sym = self.lookup(&f.name.name)?;
        let ret_ty = match &self.symbols.get(sym).ty {
            HirType::Function { ret, .. } => (**ret).clone(),
            _ => return None,
        };

        // Lower parameters into their own scope.
        self.push_scope();
        let mut params = Vec::with_capacity(f.params.len());
        for p in &f.params {
            let ty = self.lower_type(&p.ty);
            let name = p.name.as_ref().map(|n| n.name.clone()).unwrap_or_default();
            let span = p.name.as_ref().map(|n| n.span).unwrap_or(p.span);
            let psym = self.symbols.intern(hir::Symbol {
                name: name.clone(),
                kind: hir::SymbolKind::Param,
                ty: ty.clone(),
                span,
            });
            if !name.is_empty() {
                self.declare(&name, psym, span);
            }
            params.push(hir::HirParam {
                sym: psym,
                ty,
                span: p.span,
            });
        }

        let body = if let Some(body) = &f.body {
            let prev = self.current_return_type.take();
            self.current_return_type = Some(ret_ty.clone());
            let block = self.lower_block(body);
            self.current_return_type = prev;
            Some(block)
        } else {
            None
        };
        self.pop_scope();

        Some(hir::HirFnDef {
            sym,
            return_type: ret_ty,
            params,
            is_variadic: f.is_variadic,
            body,
            span: f.span,
        })
    }

    fn lower_global(&mut self, d: &ast::Decl) -> Option<hir::HirDecl> {
        let sym = self.lookup(&d.name.name)?;
        let ty = self.symbols.get(sym).ty.clone();
        let init = d.init.as_ref().map(|init| self.lower_init(&ty, init));
        Some(hir::HirDecl {
            sym,
            ty,
            init,
            span: d.span,
        })
    }

    fn lower_block(&mut self, block: &ast::Block) -> hir::HirBlock {
        self.push_scope();
        let stmts = block.stmts.iter().map(|s| self.lower_stmt(s)).collect();
        self.pop_scope();
        hir::HirBlock {
            stmts,
            span: block.span,
        }
    }

    fn lower_stmt(&mut self, s: &ast::Stmt) -> hir::HirStmt {
        let span = s.span;
        let kind = match &s.kind {
            ast::StmtKind::Compound(b) => hir::HirStmtKind::Compound(self.lower_block(b)),
            ast::StmtKind::Expr(e) => hir::HirStmtKind::Expr(self.lower_expr(e)),
            ast::StmtKind::Decl(decls) => hir::HirStmtKind::Decl(self.lower_local_decls(decls)),
            ast::StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond = self.lower_cond_expr(cond);
                let then_branch = Box::new(self.lower_stmt(then_branch));
                let else_branch = else_branch.as_ref().map(|e| Box::new(self.lower_stmt(e)));
                hir::HirStmtKind::If {
                    cond,
                    then_branch,
                    else_branch,
                }
            }
            ast::StmtKind::While { cond, body } => {
                let cond = self.lower_cond_expr(cond);
                self.loop_depth += 1;
                let body = Box::new(self.lower_stmt(body));
                self.loop_depth -= 1;
                hir::HirStmtKind::While { cond, body }
            }
            ast::StmtKind::DoWhile { body, cond } => {
                self.loop_depth += 1;
                let body = Box::new(self.lower_stmt(body));
                self.loop_depth -= 1;
                let cond = self.lower_cond_expr(cond);
                hir::HirStmtKind::DoWhile { body, cond }
            }
            ast::StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.push_scope();
                let init = match init {
                    Some(ast::ForInit::Decl(ds)) => {
                        Some(hir::HirForInit::Decl(self.lower_local_decls(ds)))
                    }
                    Some(ast::ForInit::Expr(e)) => Some(hir::HirForInit::Expr(self.lower_expr(e))),
                    None => None,
                };
                let cond = cond.as_ref().map(|c| self.lower_cond_expr(c));
                let step = step.as_ref().map(|s| self.lower_expr(s));
                self.loop_depth += 1;
                let body = Box::new(self.lower_stmt(body));
                self.loop_depth -= 1;
                self.pop_scope();
                hir::HirStmtKind::For {
                    init,
                    cond,
                    step,
                    body,
                }
            }
            ast::StmtKind::Return(opt) => {
                let expected = self.current_return_type.clone();
                let hir_expr = match (opt, expected) {
                    (Some(e), Some(ret_ty)) => {
                        let e = self.lower_expr(e);
                        let e = self.convert_to(e, &ret_ty, span);
                        Some(e)
                    }
                    (Some(e), None) => Some(self.lower_expr(e)),
                    (None, Some(ret_ty)) if !ret_ty.is_void() => {
                        self.error(
                            span,
                            E_TYPE_MISMATCH,
                            "non-void function must return a value",
                        );
                        None
                    }
                    (None, _) => None,
                };
                hir::HirStmtKind::Return(hir_expr)
            }
            ast::StmtKind::Break => {
                if self.loop_depth == 0 {
                    self.error(span, E_INVALID_OPERAND, "`break` outside of loop");
                }
                hir::HirStmtKind::Break
            }
            ast::StmtKind::Continue => {
                if self.loop_depth == 0 {
                    self.error(span, E_INVALID_OPERAND, "`continue` outside of loop");
                }
                hir::HirStmtKind::Continue
            }
            ast::StmtKind::Empty => hir::HirStmtKind::Empty,
            ast::StmtKind::Unsafe(b) => {
                // Treat the contents like a regular compound block for now.
                // The borrow checker (Phase 7) uses the lexical Unsafe marker
                // separately; in HIR we collapse to Compound so later stages
                // don't have to special-case it.
                hir::HirStmtKind::Compound(self.lower_block(b))
            }
        };
        hir::HirStmt { kind, span }
    }

    fn lower_local_decls(&mut self, decls: &[ast::Decl]) -> Vec<hir::HirDecl> {
        let mut out = Vec::with_capacity(decls.len());
        for d in decls {
            let ty = self.lower_type(&d.ty);
            let init = d.init.as_ref().map(|e| {
                let e = self.lower_expr(e);
                self.convert_to(e, &ty, d.span)
            });
            let sym = self.symbols.intern(hir::Symbol {
                name: d.name.name.clone(),
                kind: hir::SymbolKind::LocalVar,
                ty: ty.clone(),
                span: d.name.span,
            });
            self.declare(&d.name.name, sym, d.name.span);
            out.push(hir::HirDecl {
                sym,
                ty,
                init,
                span: d.span,
            });
        }
        out
    }

    fn lower_init(&mut self, target: &HirType, init: &ast::Expr) -> hir::HirExpr {
        let e = self.lower_expr(init);
        self.convert_to(e, target, init.span)
    }

    fn lower_cond_expr(&mut self, e: &ast::Expr) -> hir::HirExpr {
        let mut hir = self.lower_expr(e);
        let decayed = self.array_decay(&mut hir);
        if !decayed.is_scalar() && !decayed.is_error() {
            self.error(e.span, E_INVALID_OPERAND, "condition must have scalar type");
        }
        hir
    }

    // === Type lowering ===

    fn lower_type(&mut self, ty: &ast::Type) -> HirType {
        match &ty.kind {
            ast::TypeKind::Void => HirType::Void,
            ast::TypeKind::Bool => HirType::Bool,
            ast::TypeKind::Builtin(b) => builtin_to_hir(*b),
            ast::TypeKind::Pointer { pointee, ownership } => HirType::Pointer {
                pointee: Box::new(self.lower_type(pointee)),
                ownership: lower_ownership(*ownership),
            },
            ast::TypeKind::Array { elem, size } => {
                let elem = Box::new(self.lower_type(elem));
                // For MVP, only treat constant int literals as array sizes.
                let size_val = size.as_deref().and_then(|e| match &e.kind {
                    ast::ExprKind::IntLit(lit) => lit.raw.parse::<u64>().ok(),
                    _ => None,
                });
                HirType::Array {
                    elem,
                    size: size_val,
                }
            }
        }
    }

    // === Expressions ===

    fn lower_expr(&mut self, e: &ast::Expr) -> hir::HirExpr {
        let span = e.span;
        match &e.kind {
            ast::ExprKind::IntLit(lit) => {
                let (value, ty) = parse_int_literal(&lit.raw);
                hir::HirExpr {
                    kind: hir::HirExprKind::IntLit(value),
                    ty,
                    span,
                }
            }
            ast::ExprKind::FloatLit(lit) => {
                let value = parse_float_literal(&lit.raw);
                hir::HirExpr {
                    kind: hir::HirExprKind::FloatLit(value),
                    ty: HirType::Double,
                    span,
                }
            }
            ast::ExprKind::CharLit(s) => hir::HirExpr {
                kind: hir::HirExprKind::CharLit(parse_char_literal(s)),
                ty: HirType::Int,
                span,
            },
            ast::ExprKind::StringLit(s) => {
                let clean = s.trim_matches('"').to_string();
                hir::HirExpr {
                    kind: hir::HirExprKind::StringLit(clean),
                    ty: HirType::Pointer {
                        pointee: Box::new(HirType::Char),
                        ownership: Ownership::Raw,
                    },
                    span,
                }
            }
            ast::ExprKind::Ident(id) => match self.lookup(&id.name) {
                Some(sym) => {
                    let ty = self.symbols.get(sym).ty.clone();
                    hir::HirExpr {
                        kind: hir::HirExprKind::Ref(sym),
                        ty,
                        span,
                    }
                }
                None => {
                    self.error(
                        id.span,
                        E_UNDEFINED_IDENT,
                        format!("undefined identifier `{}`", id.name),
                    );
                    hir::HirExpr {
                        kind: hir::HirExprKind::Error,
                        ty: HirType::Error,
                        span,
                    }
                }
            },
            ast::ExprKind::Paren(inner) => self.lower_expr(inner),
            ast::ExprKind::Call { callee, args } => self.lower_call(callee, args, span),
            ast::ExprKind::Index { base, index } => self.lower_index(base, index, span),
            ast::ExprKind::Member { .. } => {
                self.error(
                    span,
                    E_INVALID_OPERAND,
                    "struct / union member access is not supported in the MVP",
                );
                hir::HirExpr {
                    kind: hir::HirExprKind::Error,
                    ty: HirType::Error,
                    span,
                }
            }
            ast::ExprKind::Unary { op, operand } => self.lower_unary(*op, operand, span),
            ast::ExprKind::Postfix { op, operand } => {
                let op = match op {
                    ast::PostfixOp::Inc => hir::HirPostfixOp::Inc,
                    ast::PostfixOp::Dec => hir::HirPostfixOp::Dec,
                };
                let mut operand_hir = self.lower_expr(operand);
                if !is_lvalue(&operand_hir) {
                    self.error(
                        operand.span,
                        E_INVALID_LVALUE,
                        "operand of postfix `++`/`--` must be an lvalue",
                    );
                }
                let ty = self.array_decay(&mut operand_hir);
                hir::HirExpr {
                    kind: hir::HirExprKind::Postfix {
                        op,
                        operand: Box::new(operand_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Binary { op, lhs, rhs } => self.lower_binary(*op, lhs, rhs, span),
            ast::ExprKind::Assign { op, lhs, rhs } => self.lower_assign(*op, lhs, rhs, span),
            ast::ExprKind::Ternary {
                cond,
                then_branch,
                else_branch,
            } => self.lower_ternary(cond, then_branch, else_branch, span),
            ast::ExprKind::Cast { ty, expr } => {
                let target = self.lower_type(ty);
                let mut inner = self.lower_expr(expr);
                self.array_decay(&mut inner);
                hir::HirExpr {
                    kind: hir::HirExprKind::Cast {
                        target: target.clone(),
                        expr: Box::new(inner),
                    },
                    ty: target,
                    span,
                }
            }
            ast::ExprKind::SizeofExpr(inner) => {
                let inner = self.lower_expr(inner);
                hir::HirExpr {
                    kind: hir::HirExprKind::SizeofExpr(Box::new(inner)),
                    ty: HirType::ULong,
                    span,
                }
            }
            ast::ExprKind::Comma(exprs) => {
                let lowered: Vec<_> = exprs.iter().map(|e| self.lower_expr(e)).collect();
                let ty = lowered
                    .last()
                    .map(|e| e.ty.clone())
                    .unwrap_or(HirType::Error);
                hir::HirExpr {
                    kind: hir::HirExprKind::Comma(lowered),
                    ty,
                    span,
                }
            }
        }
    }

    fn lower_call(&mut self, callee: &ast::Expr, args: &[ast::Expr], span: Span) -> hir::HirExpr {
        let mut callee_hir = self.lower_expr(callee);
        let callee_ty = self.array_decay(&mut callee_hir);

        let (ret_ty, param_tys, is_variadic) = match &callee_ty {
            HirType::Function {
                ret,
                params,
                is_variadic,
            } => ((**ret).clone(), params.clone(), *is_variadic),
            HirType::Pointer { pointee, .. } => match pointee.as_ref() {
                HirType::Function {
                    ret,
                    params,
                    is_variadic,
                } => ((**ret).clone(), params.clone(), *is_variadic),
                _ => {
                    if !callee_ty.is_error() {
                        self.error(
                            callee.span,
                            E_INVALID_OPERAND,
                            "called value is not a function",
                        );
                    }
                    return hir::HirExpr {
                        kind: hir::HirExprKind::Error,
                        ty: HirType::Error,
                        span,
                    };
                }
            },
            _ => {
                if !callee_ty.is_error() {
                    self.error(
                        callee.span,
                        E_INVALID_OPERAND,
                        "called value is not a function",
                    );
                }
                return hir::HirExpr {
                    kind: hir::HirExprKind::Error,
                    ty: HirType::Error,
                    span,
                };
            }
        };

        // Arity check.
        if !is_variadic && args.len() != param_tys.len() {
            self.error(
                span,
                E_ARITY_MISMATCH,
                format!(
                    "expected {} argument{}, found {}",
                    param_tys.len(),
                    if param_tys.len() == 1 { "" } else { "s" },
                    args.len()
                ),
            );
        } else if is_variadic && args.len() < param_tys.len() {
            self.error(
                span,
                E_ARITY_MISMATCH,
                format!(
                    "expected at least {} argument{}, found {}",
                    param_tys.len(),
                    if param_tys.len() == 1 { "" } else { "s" },
                    args.len()
                ),
            );
        }

        let mut hir_args = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let mut hir_arg = self.lower_expr(a);
            self.array_decay(&mut hir_arg);
            if let Some(pt) = param_tys.get(i) {
                hir_arg = self.convert_to(hir_arg, pt, a.span);
            }
            hir_args.push(hir_arg);
        }

        hir::HirExpr {
            kind: hir::HirExprKind::Call {
                callee: Box::new(callee_hir),
                args: hir_args,
            },
            ty: ret_ty,
            span,
        }
    }

    fn lower_index(&mut self, base: &ast::Expr, index: &ast::Expr, span: Span) -> hir::HirExpr {
        let mut base_hir = self.lower_expr(base);
        let base_ty = self.array_decay(&mut base_hir);
        let mut index_hir = self.lower_expr(index);
        self.array_decay(&mut index_hir);

        let elem_ty = match &base_ty {
            HirType::Pointer { pointee, .. } => (**pointee).clone(),
            _ => {
                if !base_ty.is_error() {
                    self.error(
                        base.span,
                        E_INVALID_OPERAND,
                        "subscripted value is not a pointer or array",
                    );
                }
                HirType::Error
            }
        };
        if !index_hir.ty.is_integer() && !index_hir.ty.is_error() {
            self.error(
                index.span,
                E_INVALID_OPERAND,
                "array index must be an integer",
            );
        }
        hir::HirExpr {
            kind: hir::HirExprKind::Index {
                base: Box::new(base_hir),
                index: Box::new(index_hir),
            },
            ty: elem_ty,
            span,
        }
    }

    fn lower_unary(&mut self, op: ast::UnaryOp, operand: &ast::Expr, span: Span) -> hir::HirExpr {
        let mut operand_hir = self.lower_expr(operand);
        let op_hir = match op {
            ast::UnaryOp::Plus => hir::HirUnaryOp::Plus,
            ast::UnaryOp::Neg => hir::HirUnaryOp::Neg,
            ast::UnaryOp::LogicalNot => hir::HirUnaryOp::LogicalNot,
            ast::UnaryOp::BitNot => hir::HirUnaryOp::BitNot,
            ast::UnaryOp::Deref => hir::HirUnaryOp::Deref,
            ast::UnaryOp::AddrOf => hir::HirUnaryOp::AddrOf,
            ast::UnaryOp::PreInc => hir::HirUnaryOp::PreInc,
            ast::UnaryOp::PreDec => hir::HirUnaryOp::PreDec,
        };

        let ty = match op_hir {
            hir::HirUnaryOp::AddrOf => {
                if !is_lvalue(&operand_hir) {
                    self.error(
                        operand.span,
                        E_INVALID_LVALUE,
                        "cannot take address of non-lvalue",
                    );
                }
                HirType::Pointer {
                    pointee: Box::new(operand_hir.ty.clone()),
                    ownership: Ownership::Raw,
                }
            }
            hir::HirUnaryOp::Deref => {
                let ty = self.array_decay(&mut operand_hir);
                match ty {
                    HirType::Pointer { pointee, .. } => *pointee,
                    other => {
                        if !other.is_error() {
                            self.error(
                                operand.span,
                                E_INVALID_OPERAND,
                                "dereference of non-pointer value",
                            );
                        }
                        HirType::Error
                    }
                }
            }
            hir::HirUnaryOp::LogicalNot => {
                let ty = self.array_decay(&mut operand_hir);
                if !ty.is_scalar() && !ty.is_error() {
                    self.error(
                        operand.span,
                        E_INVALID_OPERAND,
                        "operand of `!` must be scalar",
                    );
                }
                HirType::Int
            }
            hir::HirUnaryOp::BitNot => {
                let ty = self.array_decay(&mut operand_hir);
                if !ty.is_integer() && !ty.is_error() {
                    self.error(
                        operand.span,
                        E_INVALID_OPERAND,
                        "operand of `~` must be an integer",
                    );
                }
                integer_promotion(&ty)
            }
            hir::HirUnaryOp::Plus | hir::HirUnaryOp::Neg => {
                let ty = self.array_decay(&mut operand_hir);
                if !ty.is_arithmetic() && !ty.is_error() {
                    self.error(
                        operand.span,
                        E_INVALID_OPERAND,
                        "operand must be arithmetic",
                    );
                }
                integer_promotion(&ty)
            }
            hir::HirUnaryOp::PreInc | hir::HirUnaryOp::PreDec => {
                if !is_lvalue(&operand_hir) {
                    self.error(
                        operand.span,
                        E_INVALID_LVALUE,
                        "operand of prefix `++`/`--` must be an lvalue",
                    );
                }
                self.array_decay(&mut operand_hir)
            }
        };

        hir::HirExpr {
            kind: hir::HirExprKind::Unary {
                op: op_hir,
                operand: Box::new(operand_hir),
            },
            ty,
            span,
        }
    }

    fn lower_binary(
        &mut self,
        op: ast::BinaryOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
    ) -> hir::HirExpr {
        use ast::BinaryOp as B;
        let mut l = self.lower_expr(lhs);
        let mut r = self.lower_expr(rhs);
        self.array_decay(&mut l);
        self.array_decay(&mut r);

        let op_hir = match op {
            B::Mul => hir::HirBinaryOp::Mul,
            B::Div => hir::HirBinaryOp::Div,
            B::Mod => hir::HirBinaryOp::Mod,
            B::Add => hir::HirBinaryOp::Add,
            B::Sub => hir::HirBinaryOp::Sub,
            B::Shl => hir::HirBinaryOp::Shl,
            B::Shr => hir::HirBinaryOp::Shr,
            B::Lt => hir::HirBinaryOp::Lt,
            B::Gt => hir::HirBinaryOp::Gt,
            B::LtEq => hir::HirBinaryOp::LtEq,
            B::GtEq => hir::HirBinaryOp::GtEq,
            B::Eq => hir::HirBinaryOp::Eq,
            B::NotEq => hir::HirBinaryOp::NotEq,
            B::BitAnd => hir::HirBinaryOp::BitAnd,
            B::BitXor => hir::HirBinaryOp::BitXor,
            B::BitOr => hir::HirBinaryOp::BitOr,
            B::LogicalAnd => hir::HirBinaryOp::LogicalAnd,
            B::LogicalOr => hir::HirBinaryOp::LogicalOr,
        };

        let result_ty = match op {
            B::Add | B::Sub => {
                let lt = l.ty.clone();
                let rt = r.ty.clone();
                match (&lt, &rt) {
                    (HirType::Pointer { pointee, ownership }, other)
                    | (other, HirType::Pointer { pointee, ownership })
                        if op == B::Add && other.is_integer() =>
                    {
                        HirType::Pointer {
                            pointee: pointee.clone(),
                            ownership: *ownership,
                        }
                    }
                    (HirType::Pointer { pointee, ownership }, rhs_ty)
                        if op == B::Sub && rhs_ty.is_integer() =>
                    {
                        HirType::Pointer {
                            pointee: pointee.clone(),
                            ownership: *ownership,
                        }
                    }
                    (HirType::Pointer { .. }, HirType::Pointer { .. }) if op == B::Sub => {
                        HirType::Long
                    }
                    (a, b) if a.is_arithmetic() && b.is_arithmetic() => {
                        let common = hir::usual_arithmetic(a, b);
                        l = self.convert_to(l, &common, lhs.span);
                        r = self.convert_to(r, &common, rhs.span);
                        common
                    }
                    _ => {
                        self.invalid_operand(span, op_hir);
                        HirType::Error
                    }
                }
            }
            B::Mul | B::Div => {
                if l.ty.is_arithmetic() && r.ty.is_arithmetic() {
                    let common = hir::usual_arithmetic(&l.ty, &r.ty);
                    l = self.convert_to(l, &common, lhs.span);
                    r = self.convert_to(r, &common, rhs.span);
                    common
                } else {
                    if !(l.ty.is_error() || r.ty.is_error()) {
                        self.invalid_operand(span, op_hir);
                    }
                    HirType::Error
                }
            }
            B::Mod | B::Shl | B::Shr | B::BitAnd | B::BitXor | B::BitOr => {
                if l.ty.is_integer() && r.ty.is_integer() {
                    let common = hir::usual_arithmetic(&l.ty, &r.ty);
                    l = self.convert_to(l, &common, lhs.span);
                    r = self.convert_to(r, &common, rhs.span);
                    common
                } else {
                    if !(l.ty.is_error() || r.ty.is_error()) {
                        self.invalid_operand(span, op_hir);
                    }
                    HirType::Error
                }
            }
            B::Lt | B::Gt | B::LtEq | B::GtEq | B::Eq | B::NotEq => {
                if l.ty.is_arithmetic() && r.ty.is_arithmetic() {
                    let common = hir::usual_arithmetic(&l.ty, &r.ty);
                    l = self.convert_to(l, &common, lhs.span);
                    r = self.convert_to(r, &common, rhs.span);
                } else if l.ty.is_pointer() && r.ty.is_pointer() {
                    // OK for MVP.
                } else if !(l.ty.is_error() || r.ty.is_error()) {
                    self.invalid_operand(span, op_hir);
                }
                HirType::Int
            }
            B::LogicalAnd | B::LogicalOr => {
                if !l.ty.is_scalar() && !l.ty.is_error() {
                    self.invalid_operand(lhs.span, op_hir);
                }
                if !r.ty.is_scalar() && !r.ty.is_error() {
                    self.invalid_operand(rhs.span, op_hir);
                }
                HirType::Int
            }
        };

        hir::HirExpr {
            kind: hir::HirExprKind::Binary {
                op: op_hir,
                lhs: Box::new(l),
                rhs: Box::new(r),
            },
            ty: result_ty,
            span,
        }
    }

    fn invalid_operand(&mut self, span: Span, op: hir::HirBinaryOp) {
        self.error(
            span,
            E_INVALID_OPERAND,
            format!("invalid operand types for `{op:?}`"),
        );
    }

    fn lower_assign(
        &mut self,
        op: ast::AssignOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
    ) -> hir::HirExpr {
        let lhs_hir = self.lower_expr(lhs);
        if !is_lvalue(&lhs_hir) {
            self.error(
                lhs.span,
                E_INVALID_LVALUE,
                "left side of assignment must be an lvalue",
            );
        }
        let target_ty = lhs_hir.ty.clone();
        let mut rhs_hir = self.lower_expr(rhs);
        self.array_decay(&mut rhs_hir);
        let rhs_hir = self.convert_to(rhs_hir, &target_ty, rhs.span);
        let op_hir = match op {
            ast::AssignOp::Assign => hir::HirAssignOp::Assign,
            ast::AssignOp::AddAssign => hir::HirAssignOp::AddAssign,
            ast::AssignOp::SubAssign => hir::HirAssignOp::SubAssign,
            ast::AssignOp::MulAssign => hir::HirAssignOp::MulAssign,
            ast::AssignOp::DivAssign => hir::HirAssignOp::DivAssign,
            ast::AssignOp::ModAssign => hir::HirAssignOp::ModAssign,
            ast::AssignOp::ShlAssign => hir::HirAssignOp::ShlAssign,
            ast::AssignOp::ShrAssign => hir::HirAssignOp::ShrAssign,
            ast::AssignOp::AndAssign => hir::HirAssignOp::AndAssign,
            ast::AssignOp::XorAssign => hir::HirAssignOp::XorAssign,
            ast::AssignOp::OrAssign => hir::HirAssignOp::OrAssign,
        };
        hir::HirExpr {
            kind: hir::HirExprKind::Assign {
                op: op_hir,
                lhs: Box::new(lhs_hir),
                rhs: Box::new(rhs_hir),
            },
            ty: target_ty,
            span,
        }
    }

    fn lower_ternary(
        &mut self,
        cond: &ast::Expr,
        then_branch: &ast::Expr,
        else_branch: &ast::Expr,
        span: Span,
    ) -> hir::HirExpr {
        let cond_hir = self.lower_cond_expr(cond);
        let mut t = self.lower_expr(then_branch);
        let mut e = self.lower_expr(else_branch);
        self.array_decay(&mut t);
        self.array_decay(&mut e);
        let ty = if t.ty.is_arithmetic() && e.ty.is_arithmetic() {
            let common = hir::usual_arithmetic(&t.ty, &e.ty);
            t = self.convert_to(t, &common, then_branch.span);
            e = self.convert_to(e, &common, else_branch.span);
            common
        } else if t.ty == e.ty {
            t.ty.clone()
        } else if t.ty.is_error() || e.ty.is_error() {
            HirType::Error
        } else {
            // Pointer/scalar mismatch in MVP — accept but report.
            self.error(
                span,
                E_TYPE_MISMATCH,
                "ternary operands have incompatible types",
            );
            t.ty.clone()
        };
        hir::HirExpr {
            kind: hir::HirExprKind::Ternary {
                cond: Box::new(cond_hir),
                then_branch: Box::new(t),
                else_branch: Box::new(e),
            },
            ty,
            span,
        }
    }

    // === Conversion helpers ===

    fn array_decay(&mut self, expr: &mut hir::HirExpr) -> HirType {
        if let Some(decayed) = expr.ty.decay() {
            let inner = std::mem::replace(
                expr,
                hir::HirExpr {
                    kind: hir::HirExprKind::Error,
                    ty: HirType::Error,
                    span: expr.span,
                },
            );
            let new_ty = decayed.clone();
            *expr = hir::HirExpr {
                kind: hir::HirExprKind::ImplicitCast {
                    target: decayed.clone(),
                    expr: Box::new(inner),
                },
                ty: decayed,
                span: expr.span,
            };
            return new_ty;
        }
        expr.ty.clone()
    }

    fn convert_to(&mut self, mut expr: hir::HirExpr, target: &HirType, span: Span) -> hir::HirExpr {
        self.array_decay(&mut expr);
        if expr.ty == *target || expr.ty.is_error() || target.is_error() {
            return expr;
        }
        // Allow numeric <-> numeric, and any pointer to/from `void*`.
        let ok = match (&expr.ty, target) {
            (a, b) if a.is_arithmetic() && b.is_arithmetic() => true,
            (HirType::Pointer { .. }, HirType::Pointer { pointee, .. })
                if matches!(**pointee, HirType::Void) =>
            {
                true
            }
            (HirType::Pointer { pointee, .. }, HirType::Pointer { .. })
                if matches!(**pointee, HirType::Void) =>
            {
                true
            }
            _ => false,
        };
        if !ok {
            self.error(
                span,
                E_TYPE_MISMATCH,
                format!(
                    "cannot convert `{}` to `{}`",
                    hir::dump::fmt_type(&expr.ty),
                    hir::dump::fmt_type(target)
                ),
            );
            return expr;
        }
        let target_clone = target.clone();
        let new_span = expr.span;
        hir::HirExpr {
            kind: hir::HirExprKind::ImplicitCast {
                target: target_clone.clone(),
                expr: Box::new(expr),
            },
            ty: target_clone,
            span: new_span,
        }
    }
}

fn lower_ownership(o: ast::Ownership) -> Ownership {
    match o {
        ast::Ownership::Raw => Ownership::Raw,
        ast::Ownership::Owner => Ownership::Owner,
        ast::Ownership::BorrowShared => Ownership::BorrowShared,
        ast::Ownership::BorrowMut => Ownership::BorrowMut,
    }
}

fn integer_promotion(ty: &HirType) -> HirType {
    if ty.is_integer() && ty.arith_rank() < HirType::Int.arith_rank() {
        HirType::Int
    } else {
        ty.clone()
    }
}

fn is_lvalue(e: &hir::HirExpr) -> bool {
    match &e.kind {
        hir::HirExprKind::Ref(_) => true,
        hir::HirExprKind::Unary {
            op: hir::HirUnaryOp::Deref,
            ..
        } => true,
        hir::HirExprKind::Index { .. } => true,
        // Implicit casts and array decays drop lvalueness.
        _ => false,
    }
}

fn builtin_to_hir(b: ast::BuiltinType) -> HirType {
    use ast::BuiltinType as B;
    match b {
        B::Char => HirType::Char,
        B::SChar => HirType::SChar,
        B::UChar => HirType::UChar,
        B::Short => HirType::Short,
        B::UShort => HirType::UShort,
        B::Int => HirType::Int,
        B::UInt => HirType::UInt,
        B::Long => HirType::Long,
        B::ULong => HirType::ULong,
        B::LongLong => HirType::LongLong,
        B::ULongLong => HirType::ULongLong,
        B::Float => HirType::Float,
        B::Double => HirType::Double,
        B::LongDouble => HirType::LongDouble,
    }
}

/// Parse an integer literal. Returns `(value, suggested_type)`.
fn parse_int_literal(raw: &str) -> (i128, HirType) {
    let mut s = raw;
    let mut is_unsigned = false;
    let mut long_count: u32 = 0;

    // Strip suffix.
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    while end > 0 {
        let c = bytes[end - 1];
        match c {
            b'u' | b'U' => {
                is_unsigned = true;
                end -= 1;
            }
            b'l' | b'L' => {
                long_count += 1;
                end -= 1;
            }
            _ => break,
        }
    }
    s = &s[..end];

    // Strip digit separators.
    let cleaned: String = s.chars().filter(|c| *c != '\'').collect();
    let s = cleaned.as_str();

    let value = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i128::from_str_radix(rest, 16).unwrap_or(0)
    } else if let Some(rest) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        i128::from_str_radix(rest, 2).unwrap_or(0)
    } else if s.starts_with('0') && s.len() > 1 {
        i128::from_str_radix(s, 8).unwrap_or(0)
    } else {
        s.parse::<i128>().unwrap_or(0)
    };

    let ty = match (is_unsigned, long_count) {
        (false, 0) => HirType::Int,
        (true, 0) => HirType::UInt,
        (false, 1) => HirType::Long,
        (true, 1) => HirType::ULong,
        (false, _) => HirType::LongLong,
        (true, _) => HirType::ULongLong,
    };
    (value, ty)
}

fn parse_float_literal(raw: &str) -> f64 {
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(*c, '\'' | 'f' | 'F' | 'l' | 'L'))
        .collect();
    cleaned.parse::<f64>().unwrap_or(0.0)
}

fn parse_char_literal(raw: &str) -> i32 {
    // Strip optional prefix and single quotes.
    let s = raw.trim_start_matches(|c: char| c.is_alphabetic());
    let body = s.trim_matches('\'');
    let mut chars = body.chars();
    let Some(first) = chars.next() else { return 0 };
    if first == '\\' {
        let escape = chars.next().unwrap_or('\0');
        let v = match escape {
            'n' => '\n' as i32,
            't' => '\t' as i32,
            'r' => '\r' as i32,
            '\\' => '\\' as i32,
            '\'' => '\'' as i32,
            '"' => '"' as i32,
            '0' => 0,
            other => other as i32,
        };
        return v;
    }
    first as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use rccx_parser::parse_module;
    use rccx_pp::{preprocess, PpOptions};
    use rccx_source::SourceMap;

    fn check_src(src: &str) -> (hir::HirModule, Vec<Diagnostic>, SourceMap) {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", src);
        let (tokens, _) = preprocess(&mut sm, id, &PpOptions::default());
        let root_span = sm
            .file(id)
            .map(|f| Span::new(id, 0, f.text().len() as u32))
            .unwrap();
        let (module, parse_diags) = parse_module(&tokens, &sm, root_span);
        assert!(parse_diags.is_empty(), "parse errors: {parse_diags:?}");
        let (hir, diags) = check(&module);
        (hir, diags, sm)
    }

    fn dump_ok(src: &str) -> String {
        let (m, diags, _) = check_src(src);
        assert!(diags.is_empty(), "diagnostics: {diags:?}");
        hir::dump(&m)
    }

    #[test]
    fn simple_function_types_resolve() {
        let s = dump_ok("int add(int a, int b) { return a + b; }\n");
        assert!(s.contains("FnDef `add`"), "{s}");
        assert!(s.contains("Param `a`"), "{s}");
        assert!(s.contains("Binary Add : int"), "{s}");
        assert!(s.contains("Ref `a`"), "{s}");
        assert!(s.contains("Ref `b`"), "{s}");
    }

    #[test]
    fn forward_call_is_allowed() {
        let s = dump_ok(
            "int callee(void); int caller(void) { return callee(); } int callee(void) { return 0; }\n",
        );
        assert!(s.contains("Ref `callee`"), "{s}");
        assert!(s.contains("Call : int"), "{s}");
    }

    #[test]
    fn undefined_identifier_is_diagnosed() {
        let (_, diags, _) = check_src("int f(void) { return x; }\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0401"));
    }

    #[test]
    fn redefinition_is_diagnosed() {
        let (_, diags, _) = check_src("int x; int x;\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0402"));
    }

    #[test]
    fn return_type_mismatch_for_pointer_to_int() {
        let (_, diags, _) = check_src("int f(void) { int *p = 0; return p; }\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0403"));
    }

    #[test]
    fn calling_non_function_is_diagnosed() {
        let (_, diags, _) = check_src("int x; int f(void) { return x(); }\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0405"));
    }

    #[test]
    fn arity_mismatch_diagnosed() {
        let (_, diags, _) = check_src("int g(int a); int f(void) { return g(1, 2); }\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0404"));
    }

    #[test]
    fn assigning_to_rvalue_is_diagnosed() {
        let (_, diags, _) = check_src("int f(void) { 1 = 2; return 0; }\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0406"));
    }

    #[test]
    fn implicit_cast_inserted_in_initializer() {
        // `char` initialized from `int` literal should add an implicit cast.
        let s = dump_ok("int f(void) { char c = 1; return c; }\n");
        assert!(s.contains("ImplicitCast to char"), "{s}");
    }

    #[test]
    fn break_outside_loop_is_diagnosed() {
        let (_, diags, _) = check_src("int f(void) { break; return 0; }\n");
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0405"));
    }

    #[test]
    fn while_loop_types_check() {
        let s = dump_ok("int f(void) { int i = 0; while (i < 10) { i = i + 1; } return i; }\n");
        assert!(s.contains("While"), "{s}");
        assert!(s.contains("Binary Lt : int"), "{s}");
    }

    #[test]
    fn pointer_decay_for_arrays_in_index() {
        let s = dump_ok("int f(void) { int a[3]; int x = a[1]; return x; }\n");
        assert!(s.contains("ImplicitCast to int*"), "{s}");
        assert!(s.contains("Index : int"), "{s}");
    }
}
