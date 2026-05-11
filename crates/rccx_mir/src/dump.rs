//! Deterministic text dump of MIR bodies for `-emit=mir`.

use std::fmt::Write;

use rccx_hir::dump::fmt_type;

use crate::*;

pub fn dump(module: &MirModule) -> String {
    let mut out = String::new();
    for body in &module.functions {
        out.push_str(&dump_body(body));
        out.push('\n');
    }
    out
}

pub fn dump_body(body: &MirBody) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "fn `{}` #{} {{", body.name, body.sym.0);

    // Locals.
    for (i, l) in body.locals.iter().enumerate() {
        let name = l.name.as_deref().unwrap_or("_");
        let kind = match l.kind {
            LocalKind::Return => "ret",
            LocalKind::Arg => "arg",
            LocalKind::Var => "var",
            LocalKind::Temp => "tmp",
        };
        let _ = writeln!(out, "    let _{i}: {} = {kind} `{name}`;", fmt_type(&l.ty));
    }

    if !body.locals.is_empty() {
        out.push('\n');
    }

    // Blocks.
    for (bi, block) in body.blocks.iter().enumerate() {
        let _ = writeln!(out, "    bb{bi}: {{");
        for stmt in &block.statements {
            let _ = writeln!(out, "        {}", fmt_stmt(stmt));
        }
        let _ = writeln!(out, "        {};", fmt_terminator(&block.terminator));
        let _ = writeln!(out, "    }}");
    }

    let _ = writeln!(out, "}}");
    out
}

fn fmt_stmt(s: &Statement) -> String {
    match s {
        Statement::Assign(p, r) => format!("{} = {};", fmt_place(p), fmt_rvalue(r)),
        Statement::Call {
            destination,
            func,
            args,
        } => {
            let args = args.iter().map(fmt_operand).collect::<Vec<_>>().join(", ");
            let head = format!("{}({args})", fmt_operand(func));
            match destination {
                Some(p) => format!("{} = call {head};", fmt_place(p)),
                None => format!("call {head};"),
            }
        }
        Statement::StorageLive(l) => format!("StorageLive(_{});", l.0),
        Statement::StorageDead(l) => format!("StorageDead(_{});", l.0),
        Statement::Nop => "nop;".to_string(),
    }
}

fn fmt_terminator(t: &Terminator) -> String {
    match t {
        Terminator::Goto(b) => format!("goto -> bb{}", b.0),
        Terminator::SwitchInt {
            cond,
            then_block,
            else_block,
        } => format!(
            "switchInt({}) -> [nonzero: bb{}, zero: bb{}]",
            fmt_operand(cond),
            then_block.0,
            else_block.0
        ),
        Terminator::Return => "return".to_string(),
        Terminator::Drop { place, target } => {
            format!("drop({}) -> bb{}", fmt_place(place), target.0)
        }
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

fn fmt_place(p: &Place) -> String {
    let mut s = format!("_{}", p.local.0);
    for proj in &p.projections {
        match proj {
            Projection::Deref => s = format!("(*{s})"),
            Projection::Index(idx) => s = format!("{s}[_{}]", idx.0),
            Projection::Field(idx) => s = format!("{s}.f{idx}"),
        }
    }
    s
}

fn fmt_rvalue(r: &Rvalue) -> String {
    match r {
        Rvalue::Use(op) => fmt_operand(op),
        Rvalue::Ref(kind, place) => format!("&{}{}", fmt_borrow_kind(*kind), fmt_place(place)),
        Rvalue::AddressOf(kind, place) => {
            format!("&raw {}{}", fmt_borrow_kind(*kind), fmt_place(place))
        }
        Rvalue::BinaryOp(op, l, r) => {
            format!("{} {} {}", fmt_operand(l), fmt_binop(*op), fmt_operand(r))
        }
        Rvalue::UnaryOp(op, x) => format!("{}{}", fmt_unop(*op), fmt_operand(x)),
        Rvalue::Cast(kind, op, ty) => format!(
            "{}({}) as {}",
            fmt_cast_kind(*kind),
            fmt_operand(op),
            fmt_type(ty)
        ),
    }
}

fn fmt_borrow_kind(k: BorrowKind) -> &'static str {
    match k {
        BorrowKind::Shared => "",
        BorrowKind::Mut => "mut ",
    }
}

fn fmt_operand(op: &Operand) -> String {
    match op {
        Operand::Copy(p) => format!("copy {}", fmt_place(p)),
        Operand::Move(p) => format!("move {}", fmt_place(p)),
        Operand::Const(c) => fmt_constant(c),
    }
}

fn fmt_constant(c: &Constant) -> String {
    match c {
        Constant::Int(v, ty) => format!("const {v}_{}", short_type(ty)),
        Constant::Float(v, ty) => format!("const {v}_{}", short_type(ty)),
        Constant::Bool(b) => format!("const {b}"),
        Constant::Char(c) => format!("const '\\u{{{c:x}}}'"),
        Constant::Str(s) => format!("const {s:?}"),
        Constant::FnRef(sym, ty) => format!("const fn#{}_{}", sym.0, short_type(ty)),
        Constant::Null(ty) => format!("const null_{}", short_type(ty)),
        Constant::Error => "const <error>".to_string(),
    }
}

fn short_type(ty: &HirType) -> String {
    match ty {
        HirType::Void => "void".into(),
        HirType::Bool => "bool".into(),
        HirType::Char => "i8".into(),
        HirType::SChar => "i8".into(),
        HirType::UChar => "u8".into(),
        HirType::Short => "i16".into(),
        HirType::UShort => "u16".into(),
        HirType::Int => "i32".into(),
        HirType::UInt => "u32".into(),
        HirType::Long => "i64".into(),
        HirType::ULong => "u64".into(),
        HirType::LongLong => "i64".into(),
        HirType::ULongLong => "u64".into(),
        HirType::Float => "f32".into(),
        HirType::Double => "f64".into(),
        HirType::LongDouble => "f80".into(),
        HirType::Pointer { .. } => "ptr".into(),
        HirType::Array { .. } => "arr".into(),
        HirType::Function { .. } => "fn".into(),
        HirType::Error => "err".into(),
    }
}

fn fmt_binop(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::BitAnd => "&",
        BinOp::BitXor => "^",
        BinOp::BitOr => "|",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
    }
}

fn fmt_unop(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "-",
        UnOp::Not => "!",
        UnOp::BitNot => "~",
    }
}

fn fmt_cast_kind(kind: CastKind) -> &'static str {
    match kind {
        CastKind::Explicit => "cast",
        CastKind::Implicit => "icast",
        CastKind::ArrayToPointer => "arr->ptr",
    }
}
