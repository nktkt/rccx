//! AST pretty-printer used by `-emit=ast`.
//!
//! Format is deterministic and ASCII-only so it can be diffed in tests.

use std::fmt::Write;

use crate::*;

/// Render a module as an indented s-expression-like tree.
pub fn dump(module: &Module) -> String {
    let mut out = String::new();
    let mut p = Printer {
        out: &mut out,
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

    fn lineln(&mut self, prefix: &str, body: &str) {
        self.pad();
        let _ = writeln!(self.out, "{prefix}{body}");
    }

    fn indented(&mut self, f: impl FnOnce(&mut Printer)) {
        self.indent += 1;
        f(self);
        self.indent -= 1;
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::FnDef(f) => self.fn_def(f),
            Item::Decl(d) => self.decl(d, "Decl"),
        }
    }

    fn fn_def(&mut self, f: &FnDef) {
        let storage = storage_str(&f.storage);
        let label = if f.body.is_some() { "FnDef" } else { "FnDecl" };
        self.lineln(
            "",
            &format!(
                "{label} `{}` -> {}{storage}",
                f.name.name,
                fmt_type(&f.return_type)
            ),
        );
        self.indented(|p| {
            if !f.params.is_empty() || f.is_variadic {
                p.line("params:");
                p.indented(|p| {
                    for param in &f.params {
                        let label = param.name.as_ref().map(|n| n.name.as_str()).unwrap_or("_");
                        p.lineln("", &format!("Param `{}` : {}", label, fmt_type(&param.ty)));
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

    fn decl(&mut self, d: &Decl, label: &str) {
        let storage = storage_str(&d.storage);
        self.lineln(
            "",
            &format!("{label} `{}` : {}{storage}", d.name.name, fmt_type(&d.ty)),
        );
        if let Some(init) = &d.init {
            self.indented(|p| {
                p.line("init:");
                p.indented(|p| p.expr(init));
            });
        }
    }

    fn block(&mut self, block: &Block) {
        self.line("Block");
        self.indented(|p| {
            for s in &block.stmts {
                p.stmt(s);
            }
        });
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Compound(b) => self.block(b),
            StmtKind::Expr(e) => {
                self.line("ExprStmt");
                self.indented(|p| p.expr(e));
            }
            StmtKind::Decl(ds) => {
                self.line("DeclStmt");
                self.indented(|p| {
                    for d in ds {
                        p.decl(d, "Decl");
                    }
                });
            }
            StmtKind::If {
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
                    if let Some(eb) = else_branch {
                        p.line("else:");
                        p.indented(|p| p.stmt(eb));
                    }
                });
            }
            StmtKind::While { cond, body } => {
                self.line("While");
                self.indented(|p| {
                    p.line("cond:");
                    p.indented(|p| p.expr(cond));
                    p.line("body:");
                    p.indented(|p| p.stmt(body));
                });
            }
            StmtKind::DoWhile { body, cond } => {
                self.line("DoWhile");
                self.indented(|p| {
                    p.line("body:");
                    p.indented(|p| p.stmt(body));
                    p.line("cond:");
                    p.indented(|p| p.expr(cond));
                });
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.line("For");
                self.indented(|p| {
                    p.line("init:");
                    p.indented(|p| match init {
                        Some(ForInit::Decl(ds)) => {
                            for d in ds {
                                p.decl(d, "Decl");
                            }
                        }
                        Some(ForInit::Expr(e)) => p.expr(e),
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
            StmtKind::Return(opt) => {
                self.line("Return");
                if let Some(e) = opt {
                    self.indented(|p| p.expr(e));
                }
            }
            StmtKind::Break => self.line("Break"),
            StmtKind::Continue => self.line("Continue"),
            StmtKind::Empty => self.line("Empty"),
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::IntLit(lit) => self.lineln("", &format!("IntLit {}", lit.raw)),
            ExprKind::FloatLit(lit) => self.lineln("", &format!("FloatLit {}", lit.raw)),
            ExprKind::CharLit(s) => self.lineln("", &format!("CharLit {s}")),
            ExprKind::StringLit(s) => self.lineln("", &format!("StringLit {s}")),
            ExprKind::Ident(id) => self.lineln("", &format!("Ident `{}`", id.name)),
            ExprKind::Paren(inner) => {
                self.line("Paren");
                self.indented(|p| p.expr(inner));
            }
            ExprKind::Call { callee, args } => {
                self.line("Call");
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
            ExprKind::Index { base, index } => {
                self.line("Index");
                self.indented(|p| {
                    p.line("base:");
                    p.indented(|p| p.expr(base));
                    p.line("index:");
                    p.indented(|p| p.expr(index));
                });
            }
            ExprKind::Member { base, arrow, field } => {
                let arrow_s = if *arrow { "->" } else { "." };
                self.lineln("", &format!("Member {arrow_s}{}", field.name));
                self.indented(|p| p.expr(base));
            }
            ExprKind::Unary { op, operand } => {
                self.lineln("", &format!("Unary {op:?}"));
                self.indented(|p| p.expr(operand));
            }
            ExprKind::Postfix { op, operand } => {
                self.lineln("", &format!("Postfix {op:?}"));
                self.indented(|p| p.expr(operand));
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.lineln("", &format!("Binary {op:?}"));
                self.indented(|p| {
                    p.expr(lhs);
                    p.expr(rhs);
                });
            }
            ExprKind::Assign { op, lhs, rhs } => {
                self.lineln("", &format!("Assign {op:?}"));
                self.indented(|p| {
                    p.expr(lhs);
                    p.expr(rhs);
                });
            }
            ExprKind::Ternary {
                cond,
                then_branch,
                else_branch,
            } => {
                self.line("Ternary");
                self.indented(|p| {
                    p.expr(cond);
                    p.expr(then_branch);
                    p.expr(else_branch);
                });
            }
            ExprKind::Cast { ty, expr } => {
                self.lineln("", &format!("Cast to {}", fmt_type(ty)));
                self.indented(|p| p.expr(expr));
            }
            ExprKind::SizeofExpr(inner) => {
                self.line("SizeofExpr");
                self.indented(|p| p.expr(inner));
            }
            ExprKind::Comma(exprs) => {
                self.line("Comma");
                self.indented(|p| {
                    for e in exprs {
                        p.expr(e);
                    }
                });
            }
        }
    }
}

fn fmt_type(ty: &Type) -> String {
    let mut out = String::new();
    fmt_type_into(ty, &mut out);
    out
}

fn fmt_type_into(ty: &Type, out: &mut String) {
    let q = ty.qualifiers;
    if q.is_const {
        out.push_str("const ");
    }
    if q.is_volatile {
        out.push_str("volatile ");
    }
    if q.is_restrict {
        out.push_str("restrict ");
    }
    if q.is_atomic {
        out.push_str("_Atomic ");
    }
    match &ty.kind {
        TypeKind::Void => out.push_str("void"),
        TypeKind::Bool => out.push_str("bool"),
        TypeKind::Builtin(b) => out.push_str(builtin_name(*b)),
        TypeKind::Pointer(inner) => {
            fmt_type_into(inner, out);
            out.push('*');
        }
        TypeKind::Array { elem, size } => {
            fmt_type_into(elem, out);
            match size.as_deref() {
                Some(_) => out.push_str("[N]"),
                None => out.push_str("[]"),
            }
        }
    }
}

fn builtin_name(b: BuiltinType) -> &'static str {
    match b {
        BuiltinType::Char => "char",
        BuiltinType::SChar => "signed char",
        BuiltinType::UChar => "unsigned char",
        BuiltinType::Short => "short",
        BuiltinType::UShort => "unsigned short",
        BuiltinType::Int => "int",
        BuiltinType::UInt => "unsigned int",
        BuiltinType::Long => "long",
        BuiltinType::ULong => "unsigned long",
        BuiltinType::LongLong => "long long",
        BuiltinType::ULongLong => "unsigned long long",
        BuiltinType::Float => "float",
        BuiltinType::Double => "double",
        BuiltinType::LongDouble => "long double",
    }
}

fn storage_str(specs: &[StorageSpec]) -> String {
    if specs.is_empty() {
        return String::new();
    }
    let mut out = String::from(" [");
    for (i, s) in specs.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(match s {
            StorageSpec::Static => "static",
            StorageSpec::Extern => "extern",
            StorageSpec::Auto => "auto",
            StorageSpec::Register => "register",
            StorageSpec::Inline => "inline",
            StorageSpec::Noreturn => "noreturn",
            StorageSpec::ThreadLocal => "thread_local",
        });
    }
    out.push(']');
    out
}
