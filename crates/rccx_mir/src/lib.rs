//! MIR-C: a control-flow-graph IR for `rccx`, built from HIR.
//!
//! Phase 5 MVP. Modeled after Rust's MIR but adapted for C semantics:
//!
//! - Bodies are split into [`BasicBlock`]s, each ending in a [`Terminator`].
//! - Statements operate on [`Place`]s (lvalues) and [`Rvalue`]s.
//! - Calls live as statements; in this MVP we don't model unwinding, so
//!   they don't need to be terminators.
//! - `Move` operands are emitted for [`HirType::Pointer`] reads so that the
//!   future borrow checker can see ownership transfers; everything else uses
//!   `Copy`. (This is an approximation — the real `[[sc::owner]]` rules will
//!   refine it in Phase 7.)
//!
//! Out of scope for the MVP (Phase 5.x):
//!
//! - `switch` / `case`, `goto` / labels.
//! - struct / union field projections (we only generate `Field` projections
//!   when the source program is empty of structs — i.e. never, today).
//! - true SSA form. Re-assignment of the same local is allowed.
//! - Drop / Free elaboration. We reserve those terminators so the borrow
//!   checker can be wired in later, but the builder does not emit them.

pub mod build;
pub mod dump;

use std::collections::HashMap;

use rccx_hir::{HirType, SymbolId};
use rccx_source::Span;

pub use build::build_module;
pub use dump::{dump, dump_body};

/// All MIR bodies in a translation unit.
#[derive(Debug, Clone, Default)]
pub struct MirModule {
    /// Function bodies. Order matches the order they appear in the HIR.
    pub functions: Vec<MirBody>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

impl BlockId {
    pub const START: BlockId = BlockId(0);
}

/// A function body in MIR form.
#[derive(Debug, Clone)]
pub struct MirBody {
    /// Symbol of the function being defined.
    pub sym: SymbolId,
    /// Pretty name (resolved from the symbol table when dumping).
    pub name: String,
    /// `locals[0]` is the implicit return slot. Parameters follow, then
    /// declared locals, then compiler-generated temporaries.
    pub locals: Vec<LocalDecl>,
    /// `LocalId`s of the parameters, in source order. Always
    /// `LocalId(1)..=LocalId(arg_count)`.
    pub arg_count: u32,
    /// Control-flow graph. `blocks[0]` is the entry point.
    pub blocks: Vec<BasicBlock>,
    pub span: Span,
}

impl MirBody {
    pub fn return_local(&self) -> LocalId {
        LocalId(0)
    }

    pub fn local(&self, id: LocalId) -> &LocalDecl {
        &self.locals[id.0 as usize]
    }
}

#[derive(Debug, Clone)]
pub struct LocalDecl {
    pub ty: HirType,
    pub name: Option<String>,
    pub kind: LocalKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Return,
    Arg,
    Var,
    Temp,
}

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Terminator,
}

impl BasicBlock {
    pub fn new(terminator: Terminator) -> Self {
        Self {
            statements: Vec::new(),
            terminator,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Statement {
    Assign(Place, Rvalue),
    /// `dest = func(args...)`. `dest` is `None` for void calls.
    Call {
        destination: Option<Place>,
        func: Operand,
        args: Vec<Operand>,
    },
    /// `dest` is freshly initialized; future borrow checker treats this as
    /// the start of `dest`'s lifetime.
    StorageLive(LocalId),
    /// `dest` is being released; future borrow checker treats this as the
    /// end of `dest`'s lifetime.
    StorageDead(LocalId),
    Nop,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Terminator {
    Goto(BlockId),
    /// Two-way branch on a scalar operand.
    ///
    /// `cond` is treated as a boolean: non-zero → `then_block`, zero →
    /// `else_block`. This is enough for `if`/`while`/`for` lowering;
    /// multi-target `switch` is planned for Phase 5.x.
    SwitchInt {
        cond: Operand,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return,
    /// Reserved for the borrow checker / codegen. Builders never emit this in
    /// Phase 5 MVP.
    #[allow(dead_code)]
    Drop {
        place: Place,
        target: BlockId,
    },
    /// Statically unreachable point (e.g. after a `return`).
    Unreachable,
}

#[derive(Debug, Clone)]
pub struct Place {
    pub local: LocalId,
    pub projections: Vec<Projection>,
}

impl Place {
    pub fn from_local(local: LocalId) -> Self {
        Self {
            local,
            projections: Vec::new(),
        }
    }

    pub fn with_deref(mut self) -> Self {
        self.projections.push(Projection::Deref);
        self
    }

    pub fn with_index(mut self, index: LocalId) -> Self {
        self.projections.push(Projection::Index(index));
        self
    }
}

#[derive(Debug, Clone)]
pub enum Projection {
    Deref,
    /// `place[local]` — the index is required to live in a `Local` so the
    /// borrow checker can reason about it without a sub-IR.
    Index(LocalId),
    /// Reserved for struct field access (Phase 5.x).
    #[allow(dead_code)]
    Field(u32),
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Rvalue {
    Use(Operand),
    /// `&place` (shared) or `&mut place`.
    Ref(BorrowKind, Place),
    /// `*place`-like: cast a pointer-typed operand back to a value.
    AddressOf(BorrowKind, Place),
    BinaryOp(BinOp, Operand, Operand),
    UnaryOp(UnOp, Operand),
    Cast(CastKind, Operand, HirType),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowKind {
    Shared,
    Mut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    BitAnd,
    BitXor,
    BitOr,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    Explicit,
    Implicit,
    /// Array-to-pointer decay.
    ArrayToPointer,
}

#[derive(Debug, Clone)]
pub enum Operand {
    /// Read the value of `place`. Bitwise copy semantics.
    Copy(Place),
    /// Read `place` with move semantics. Phase 5 MVP emits this for any
    /// read of an owner-typed value (currently approximated as "any pointer
    /// read", since `[[sc::owner]]` plumbing arrives in Phase 7).
    Move(Place),
    /// Compile-time-known constant.
    Const(Constant),
}

#[derive(Debug, Clone)]
pub enum Constant {
    Int(i128, HirType),
    Float(f64, HirType),
    Bool(bool),
    Char(i32),
    /// String literal contents, without surrounding quotes.
    Str(String),
    /// `&function`. Used as the callee operand of `Call`.
    FnRef(SymbolId, HirType),
    Null(HirType),
    /// Type-erased placeholder used during error recovery.
    Error,
}

/// Helper used by the builder: map from HIR `SymbolId` to MIR `LocalId` for
/// per-body local lookups.
pub(crate) type SymbolLocals = HashMap<SymbolId, LocalId>;

#[cfg(test)]
mod tests {
    use super::*;
    use rccx_parser::parse_module;
    use rccx_pp::{preprocess, PpOptions};
    use rccx_source::{SourceMap, Span};
    use rccx_typeck::check as typeck_check;

    fn mir_from(src: &str) -> MirModule {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", src);
        let (tokens, _) = preprocess(&mut sm, id, &PpOptions::default());
        let root_span = sm
            .file(id)
            .map(|f| Span::new(id, 0, f.text().len() as u32))
            .unwrap();
        let (module, parse_diags) = parse_module(&tokens, &sm, root_span);
        assert!(parse_diags.is_empty(), "parse: {parse_diags:?}");
        let (hir, type_diags) = typeck_check(&module);
        assert!(type_diags.is_empty(), "typeck: {type_diags:?}");
        build_module(&hir)
    }

    #[test]
    fn return_only_function_has_entry_return() {
        let m = mir_from("int main(void) { return 0; }\n");
        assert_eq!(m.functions.len(), 1);
        let body = &m.functions[0];
        // Return slot + no args.
        assert_eq!(body.arg_count, 0);
        assert_eq!(body.locals[0].kind, LocalKind::Return);
        // Entry block must end with Return.
        assert!(matches!(body.blocks[0].terminator, Terminator::Return));
    }

    #[test]
    fn binary_expression_lowers_to_temporary() {
        let m = mir_from("int add(int a, int b) { return a + b; }\n");
        let body = &m.functions[0];
        let dump = dump_body(body);
        assert!(dump.contains("+ "), "{dump}");
        assert!(dump.contains("return"), "{dump}");
    }

    #[test]
    fn if_else_creates_branch_and_join() {
        let m = mir_from("int f(int x) { if (x) { return 1; } else { return 2; } }\n");
        let body = &m.functions[0];
        let has_switch = body
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::SwitchInt { .. }));
        assert!(has_switch, "expected a SwitchInt terminator");
        let return_count = body
            .blocks
            .iter()
            .filter(|b| matches!(b.terminator, Terminator::Return))
            .count();
        assert!(return_count >= 2, "expected at least two Return blocks");
    }

    #[test]
    fn while_creates_loop_header_and_back_edge() {
        let m = mir_from("int f(int n) { while (n) { n = n - 1; } return n; }\n");
        let body = &m.functions[0];
        let dump = dump_body(body);
        // Header block should be reachable from a goto inside the loop body.
        let goto_count = body
            .blocks
            .iter()
            .filter(|b| matches!(b.terminator, Terminator::Goto(_)))
            .count();
        assert!(goto_count >= 2, "{dump}");
        let has_switch = body
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::SwitchInt { .. }));
        assert!(has_switch, "{dump}");
    }

    #[test]
    fn break_in_loop_targets_exit_block() {
        let m = mir_from("int f(void) { while (1) { break; } return 0; }\n");
        let body = &m.functions[0];
        // A break followed by a return should still produce a Return-terminated
        // block.
        assert!(body
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Return)));
    }

    #[test]
    fn raw_pointer_read_uses_copy() {
        let m = mir_from("int f(void) { int x = 1; int *p = &x; int y = *p; return y; }\n");
        let body = &m.functions[0];
        let dump = dump_body(body);
        // Raw pointers (no Safe C ownership attribute) read by copy.
        assert!(dump.contains("&raw "), "{dump}");
        assert!(!dump.contains("move "), "{dump}");
    }

    #[test]
    fn owner_pointer_read_uses_move() {
        let m = mir_from(
            "void consume([[sc::owner]] int *p);\nvoid f(void) { [[sc::owner]] int *p; consume(p); }\n",
        );
        // Prototype has no body, so we get one MIR for `f`.
        assert_eq!(m.functions.len(), 1);
        let dump = dump_body(&m.functions[0]);
        assert!(dump.contains("move "), "{dump}");
    }

    #[test]
    fn call_statement_records_destination() {
        let m = mir_from("int g(int);\nint f(void) { int x = g(3); return x; }\n");
        let body = &m.functions[0]; // first body is `f`
        let dump = dump_body(body);
        assert!(dump.contains("= call "), "{dump}");
    }

    #[test]
    fn logical_and_short_circuits_via_switch() {
        let m = mir_from("int f(int x, int y) { return x && y; }\n");
        let body = &m.functions[0];
        let switches = body
            .blocks
            .iter()
            .filter(|b| matches!(b.terminator, Terminator::SwitchInt { .. }))
            .count();
        // One switch for the && and (no outer if), so >= 1 — but we expect
        // exactly one introduced by the short-circuit lowering.
        assert!(switches >= 1, "expected a SwitchInt for &&");
    }
}
