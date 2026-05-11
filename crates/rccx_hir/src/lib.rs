//! Typed, name-resolved IR for `rccx`.
//!
//! Produced by `rccx_typeck` from an `ast::Module`. Every identifier reference
//! is a `SymbolId`, every expression carries a [`HirType`], and any implicit
//! conversions inserted by the type checker show up as [`HirExprKind::ImplicitCast`].
//!
//! Out of scope for the MVP (planned for Phase 4.x):
//!
//! - `typedef` / `struct` / `union` / `enum` type resolution.
//! - Function pointer types.
//! - Bit-fields, designated initializers.
//! - Full set of C's "usual arithmetic conversion" rules. We approximate by
//!   promoting to whichever operand has the higher integer / floating rank,
//!   defaulting to `int` for narrow integer types.

pub mod dump;

use rccx_source::Span;

pub use dump::dump;

// === Symbols =============================================================

/// Stable identifier for a symbol inside a [`SymbolTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub u32);

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub ty: HirType,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    GlobalVar,
    LocalVar,
    Param,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    pub symbols: Vec<Symbol>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, sym: Symbol) -> SymbolId {
        let id = SymbolId(self.symbols.len() as u32);
        self.symbols.push(sym);
        id
    }

    pub fn get(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: SymbolId) -> &mut Symbol {
        &mut self.symbols[id.0 as usize]
    }
}

// === Module / items ======================================================

#[derive(Debug, Clone)]
pub struct HirModule {
    pub items: Vec<HirItem>,
    pub symbols: SymbolTable,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirItem {
    FnDef(HirFnDef),
    Decl(HirDecl),
}

impl HirItem {
    pub fn span(&self) -> Span {
        match self {
            HirItem::FnDef(f) => f.span,
            HirItem::Decl(d) => d.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HirFnDef {
    pub sym: SymbolId,
    pub return_type: HirType,
    pub params: Vec<HirParam>,
    pub is_variadic: bool,
    /// `None` for a prototype.
    pub body: Option<HirBlock>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirParam {
    pub sym: SymbolId,
    pub ty: HirType,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirDecl {
    pub sym: SymbolId,
    pub ty: HirType,
    pub init: Option<HirExpr>,
    pub span: Span,
}

// === Statements ==========================================================

#[derive(Debug, Clone)]
pub struct HirBlock {
    pub stmts: Vec<HirStmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirStmt {
    pub kind: HirStmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum HirStmtKind {
    Compound(HirBlock),
    Expr(HirExpr),
    Decl(Vec<HirDecl>),
    If {
        cond: HirExpr,
        then_branch: Box<HirStmt>,
        else_branch: Option<Box<HirStmt>>,
    },
    While {
        cond: HirExpr,
        body: Box<HirStmt>,
    },
    DoWhile {
        body: Box<HirStmt>,
        cond: HirExpr,
    },
    For {
        init: Option<HirForInit>,
        cond: Option<HirExpr>,
        step: Option<HirExpr>,
        body: Box<HirStmt>,
    },
    Return(Option<HirExpr>),
    Break,
    Continue,
    Empty,
}

#[derive(Debug, Clone)]
pub enum HirForInit {
    Decl(Vec<HirDecl>),
    Expr(HirExpr),
}

// === Expressions =========================================================

#[derive(Debug, Clone)]
pub struct HirExpr {
    pub kind: HirExprKind,
    pub ty: HirType,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirExprKind {
    /// Integer constant, value already parsed.
    IntLit(i128),
    /// Floating constant, value already parsed.
    FloatLit(f64),
    /// Character constant; stores its integer value.
    CharLit(i32),
    /// String literal; stores the source slice (with quotes stripped).
    StringLit(String),
    /// Reference to a resolved symbol.
    Ref(SymbolId),
    Call {
        callee: Box<HirExpr>,
        args: Vec<HirExpr>,
    },
    Index {
        base: Box<HirExpr>,
        index: Box<HirExpr>,
    },
    Unary {
        op: HirUnaryOp,
        operand: Box<HirExpr>,
    },
    Postfix {
        op: HirPostfixOp,
        operand: Box<HirExpr>,
    },
    Binary {
        op: HirBinaryOp,
        lhs: Box<HirExpr>,
        rhs: Box<HirExpr>,
    },
    Assign {
        op: HirAssignOp,
        lhs: Box<HirExpr>,
        rhs: Box<HirExpr>,
    },
    Ternary {
        cond: Box<HirExpr>,
        then_branch: Box<HirExpr>,
        else_branch: Box<HirExpr>,
    },
    /// User-written cast.
    Cast {
        target: HirType,
        expr: Box<HirExpr>,
    },
    /// Implicit conversion inserted by the type checker.
    ImplicitCast {
        target: HirType,
        expr: Box<HirExpr>,
    },
    SizeofExpr(Box<HirExpr>),
    Comma(Vec<HirExpr>),
    /// Placeholder for expressions the type checker could not handle.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirUnaryOp {
    Plus,
    Neg,
    LogicalNot,
    BitNot,
    Deref,
    AddrOf,
    PreInc,
    PreDec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirPostfixOp {
    Inc,
    Dec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirBinaryOp {
    Mul,
    Div,
    Mod,
    Add,
    Sub,
    Shl,
    Shr,
    Lt,
    Gt,
    LtEq,
    GtEq,
    Eq,
    NotEq,
    BitAnd,
    BitXor,
    BitOr,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirAssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    ShlAssign,
    ShrAssign,
    AndAssign,
    XorAssign,
    OrAssign,
}

// === Types ===============================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirType {
    Void,
    Bool,
    Char,
    SChar,
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Long,
    ULong,
    LongLong,
    ULongLong,
    Float,
    Double,
    LongDouble,
    Pointer(Box<HirType>),
    Array {
        elem: Box<HirType>,
        /// `None` for incomplete arrays.
        size: Option<u64>,
    },
    Function {
        ret: Box<HirType>,
        params: Vec<HirType>,
        is_variadic: bool,
    },
    /// Placeholder for type errors so the rest of the checker can keep walking.
    Error,
}

impl HirType {
    pub fn is_error(&self) -> bool {
        matches!(self, HirType::Error)
    }

    pub fn is_void(&self) -> bool {
        matches!(self, HirType::Void)
    }

    pub fn is_pointer(&self) -> bool {
        matches!(self, HirType::Pointer(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, HirType::Array { .. })
    }

    pub fn is_function(&self) -> bool {
        matches!(self, HirType::Function { .. })
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            HirType::Bool
                | HirType::Char
                | HirType::SChar
                | HirType::UChar
                | HirType::Short
                | HirType::UShort
                | HirType::Int
                | HirType::UInt
                | HirType::Long
                | HirType::ULong
                | HirType::LongLong
                | HirType::ULongLong
        )
    }

    pub fn is_floating(&self) -> bool {
        matches!(self, HirType::Float | HirType::Double | HirType::LongDouble)
    }

    pub fn is_arithmetic(&self) -> bool {
        self.is_integer() || self.is_floating()
    }

    pub fn is_scalar(&self) -> bool {
        self.is_arithmetic() || self.is_pointer()
    }

    /// Integer-promotion rank (higher = wider). Used by the MVP "usual
    /// arithmetic conversion" approximation.
    pub fn arith_rank(&self) -> u8 {
        match self {
            HirType::LongDouble => 14,
            HirType::Double => 13,
            HirType::Float => 12,
            HirType::ULongLong => 11,
            HirType::LongLong => 10,
            HirType::ULong => 9,
            HirType::Long => 8,
            HirType::UInt => 7,
            HirType::Int => 6,
            HirType::UShort => 5,
            HirType::Short => 4,
            HirType::UChar => 3,
            HirType::SChar => 2,
            HirType::Char => 2,
            HirType::Bool => 1,
            _ => 0,
        }
    }

    /// Result of array-to-pointer decay. Returns `None` for non-arrays.
    pub fn decay(&self) -> Option<HirType> {
        match self {
            HirType::Array { elem, .. } => Some(HirType::Pointer(elem.clone())),
            _ => None,
        }
    }
}

/// Apply the MVP approximation of C's "usual arithmetic conversion" to two
/// arithmetic types.
pub fn usual_arithmetic(a: &HirType, b: &HirType) -> HirType {
    if !a.is_arithmetic() || !b.is_arithmetic() {
        return HirType::Error;
    }
    // Integer promotion: any sub-int integer promotes to int first.
    let promote = |t: &HirType| -> HirType {
        if t.is_integer() && t.arith_rank() < HirType::Int.arith_rank() {
            HirType::Int
        } else {
            t.clone()
        }
    };
    let pa = promote(a);
    let pb = promote(b);
    if pa == pb {
        return pa;
    }
    if pa.arith_rank() >= pb.arith_rank() {
        pa
    } else {
        pb
    }
}
