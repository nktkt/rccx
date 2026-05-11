//! HIR pretty-printer used by `-emit=hir`.

use std::fmt::Write;

use crate::*;

pub fn dump(module: &HirModule) -> String {
    let mut out = String::new();
    let mut p = Printer {
        out: &mut out,
        module,
        indent: 0,
    };
    p.line("Module");
    p.indented(|p| {
        for item in &module.items {
            p.item(item);
        }
    });
    out
}

struct Printer<'a> {
    out: &'a mut String,
    module: &'a HirModule,
    indent: usize,
}

impl<'a> Printer<'a> {
    fn pad(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
    }

    fn line(&mut self, s: &str) {
        self.pad();
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn writeln_args(&mut self, args: std::fmt::Arguments<'_>) {
        self.pad();
        let _ = self.out.write_fmt(args);
        self.out.push('\n');
    }

    fn indented(&mut self, f: impl FnOnce(&mut Printer)) {
        self.indent += 1;
        f(self);
        self.indent -= 1;
    }

    fn item(&mut self, item: &HirItem) {
        match item {
            HirItem::FnDef(f) => self.fn_def(f),
            HirItem::Decl(d) => {
                let sym = self.module.symbols.get(d.sym);
                self.writeln_args(format_args!(
                    "Decl `{}` #{} : {}",
                    sym.name,
                    d.sym.0,
                    fmt_type(&d.ty)
                ));
                if let Some(init) = &d.init {
                    self.indented(|p| {
                        p.line("init:");
                        p.indented(|p| p.expr(init));
                    });
                }
            }
        }
    }

    fn fn_def(&mut self, f: &HirFnDef) {
        let sym = self.module.symbols.get(f.sym);
        let label = if f.body.is_some() { "FnDef" } else { "FnDecl" };
        self.writeln_args(format_args!(
            "{label} `{}` #{} -> {}",
            sym.name,
            f.sym.0,
            fmt_type(&f.return_type)
        ));
        self.indented(|p| {
            if !f.params.is_empty() || f.is_variadic {
                p.line("params:");
                p.indented(|p| {
                    for param in &f.params {
                        let psym = p.module.symbols.get(param.sym);
                        p.writeln_args(format_args!(
                            "Param `{}` #{} : {}",
                            psym.name,
                            param.sym.0,
                            fmt_type(&param.ty)
                        ));
                    }
                    if f.is_variadic {
                        p.line("...");
                    }
                });
            }
            if let Some(body) = &f.body {
                p.line("body:");
                p.indented(|p| p.block(body));
            }
        });
    }

    fn block(&mut self, block: &HirBlock) {
        self.line("Block");
        self.indented(|p| {
            for s in &block.stmts {
                p.stmt(s);
            }
        });
    }

    fn stmt(&mut self, s: &HirStmt) {
        match &s.kind {
            HirStmtKind::Compound(b) => self.block(b),
            HirStmtKind::Expr(e) => {
                self.line("ExprStmt");
                self.indented(|p| p.expr(e));
            }
            HirStmtKind::Decl(decls) => {
                self.line("DeclStmt");
                self.indented(|p| {
                    for d in decls {
                        let sym = p.module.symbols.get(d.sym);
                        p.writeln_args(format_args!(
                            "Decl `{}` #{} : {}",
                            sym.name,
                            d.sym.0,
                            fmt_type(&d.ty)
                        ));
                        if let Some(init) = &d.init {
                            p.indented(|p| {
                                p.line("init:");
                                p.indented(|p| p.expr(init));
                            });
                        }
                    }
                });
            }
            HirStmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.line("If");
                self.indented(|p| {
                    p.line("cond:");
                    p.indented(|p| p.expr(cond));
                    p.line("then:");
                    p.indented(|p| p.stmt(then_branch));
                    if let Some(e) = else_branch {
                        p.line("else:");
                        p.indented(|p| p.stmt(e));
                    }
                });
            }
            HirStmtKind::While { cond, body } => {
                self.line("While");
                self.indented(|p| {
                    p.line("cond:");
                    p.indented(|p| p.expr(cond));
                    p.line("body:");
                    p.indented(|p| p.stmt(body));
                });
            }
            HirStmtKind::DoWhile { body, cond } => {
                self.line("DoWhile");
                self.indented(|p| {
                    p.line("body:");
                    p.indented(|p| p.stmt(body));
                    p.line("cond:");
                    p.indented(|p| p.expr(cond));
                });
            }
            HirStmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.line("For");
                self.indented(|p| {
                    p.line("init:");
                    p.indented(|p| match init {
                        Some(HirForInit::Decl(ds)) => {
                            for d in ds {
                                let sym = p.module.symbols.get(d.sym);
                                p.writeln_args(format_args!(
                                    "Decl `{}` #{} : {}",
                                    sym.name,
                                    d.sym.0,
                                    fmt_type(&d.ty)
                                ));
                                if let Some(init) = &d.init {
                                    p.indented(|p| {
                                        p.line("init:");
                                        p.indented(|p| p.expr(init));
                                    });
                                }
                            }
                        }
                        Some(HirForInit::Expr(e)) => p.expr(e),
                        None => p.line("<none>"),
                    });
                    p.line("cond:");
                    p.indented(|p| match cond {
                        Some(c) => p.expr(c),
                        None => p.line("<none>"),
                    });
                    p.line("step:");
                    p.indented(|p| match step {
                        Some(c) => p.expr(c),
                        None => p.line("<none>"),
                    });
                    p.line("body:");
                    p.indented(|p| p.stmt(body));
                });
            }
            HirStmtKind::Return(opt) => {
                self.line("Return");
                if let Some(e) = opt {
                    self.indented(|p| p.expr(e));
                }
            }
            HirStmtKind::Break => self.line("Break"),
            HirStmtKind::Continue => self.line("Continue"),
            HirStmtKind::Empty => self.line("Empty"),
        }
    }

    fn expr(&mut self, e: &HirExpr) {
        let ty = fmt_type(&e.ty);
        match &e.kind {
            HirExprKind::IntLit(v) => self.writeln_args(format_args!("IntLit {v} : {ty}")),
            HirExprKind::FloatLit(v) => self.writeln_args(format_args!("FloatLit {v} : {ty}")),
            HirExprKind::CharLit(v) => self.writeln_args(format_args!("CharLit {v} : {ty}")),
            HirExprKind::StringLit(s) => self.writeln_args(format_args!("StringLit {s:?} : {ty}")),
            HirExprKind::Ref(id) => {
                let sym = self.module.symbols.get(*id);
                self.writeln_args(format_args!("Ref `{}` #{} : {ty}", sym.name, id.0));
            }
            HirExprKind::Call { callee, args } => {
                self.writeln_args(format_args!("Call : {ty}"));
                self.indented(|p| {
                    p.line("callee:");
                    p.indented(|p| p.expr(callee));
                    p.line("args:");
                    p.indented(|p| {
                        if args.is_empty() {
                            p.line("<none>");
                        }
                        for a in args {
                            p.expr(a);
                        }
                    });
                });
            }
            HirExprKind::Index { base, index } => {
                self.writeln_args(format_args!("Index : {ty}"));
                self.indented(|p| {
                    p.expr(base);
                    p.expr(index);
                });
            }
            HirExprKind::Unary { op, operand } => {
                self.writeln_args(format_args!("Unary {op:?} : {ty}"));
                self.indented(|p| p.expr(operand));
            }
            HirExprKind::Postfix { op, operand } => {
                self.writeln_args(format_args!("Postfix {op:?} : {ty}"));
                self.indented(|p| p.expr(operand));
            }
            HirExprKind::Binary { op, lhs, rhs } => {
                self.writeln_args(format_args!("Binary {op:?} : {ty}"));
                self.indented(|p| {
                    p.expr(lhs);
                    p.expr(rhs);
                });
            }
            HirExprKind::Assign { op, lhs, rhs } => {
                self.writeln_args(format_args!("Assign {op:?} : {ty}"));
                self.indented(|p| {
                    p.expr(lhs);
                    p.expr(rhs);
                });
            }
            HirExprKind::Ternary {
                cond,
                then_branch,
                else_branch,
            } => {
                self.writeln_args(format_args!("Ternary : {ty}"));
                self.indented(|p| {
                    p.expr(cond);
                    p.expr(then_branch);
                    p.expr(else_branch);
                });
            }
            HirExprKind::Cast { target, expr } => {
                self.writeln_args(format_args!("Cast to {} : {ty}", fmt_type(target)));
                self.indented(|p| p.expr(expr));
            }
            HirExprKind::ImplicitCast { target, expr } => {
                self.writeln_args(format_args!("ImplicitCast to {} : {ty}", fmt_type(target)));
                self.indented(|p| p.expr(expr));
            }
            HirExprKind::SizeofExpr(inner) => {
                self.writeln_args(format_args!("SizeofExpr : {ty}"));
                self.indented(|p| p.expr(inner));
            }
            HirExprKind::Comma(exprs) => {
                self.writeln_args(format_args!("Comma : {ty}"));
                self.indented(|p| {
                    for e in exprs {
                        p.expr(e);
                    }
                });
            }
            HirExprKind::Error => self.writeln_args(format_args!("<error> : {ty}")),
        }
    }
}

pub fn fmt_type(ty: &HirType) -> String {
    match ty {
        HirType::Void => "void".into(),
        HirType::Bool => "bool".into(),
        HirType::Char => "char".into(),
        HirType::SChar => "signed char".into(),
        HirType::UChar => "unsigned char".into(),
        HirType::Short => "short".into(),
        HirType::UShort => "unsigned short".into(),
        HirType::Int => "int".into(),
        HirType::UInt => "unsigned int".into(),
        HirType::Long => "long".into(),
        HirType::ULong => "unsigned long".into(),
        HirType::LongLong => "long long".into(),
        HirType::ULongLong => "unsigned long long".into(),
        HirType::Float => "float".into(),
        HirType::Double => "double".into(),
        HirType::LongDouble => "long double".into(),
        HirType::Pointer { pointee, ownership } => {
            let base = fmt_type(pointee);
            match ownership {
                crate::Ownership::Raw => format!("{base}*"),
                crate::Ownership::Owner => format!("{base}*owner"),
                crate::Ownership::BorrowShared => format!("{base}*borrow"),
                crate::Ownership::BorrowMut => format!("{base}*borrow_mut"),
            }
        }
        HirType::Array { elem, size } => match size {
            Some(n) => format!("{}[{n}]", fmt_type(elem)),
            None => format!("{}[]", fmt_type(elem)),
        },
        HirType::Function {
            ret,
            params,
            is_variadic,
        } => {
            let mut s = format!("{} (", fmt_type(ret));
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&fmt_type(p));
            }
            if *is_variadic {
                if !params.is_empty() {
                    s.push_str(", ");
                }
                s.push_str("...");
            }
            s.push(')');
            s
        }
        HirType::Error => "<error>".into(),
    }
}
