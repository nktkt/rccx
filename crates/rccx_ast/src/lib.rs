//! AST for `rccx` — Phase 3 MVP.
//!
//! Every node carries a [`Span`] pointing back into the source map so later
//! stages and diagnostics can report locations precisely.
//!
//! Out of scope for the MVP (planned for Phase 3.x):
//!
//! - `struct` / `union` / `enum` declarations.
//! - `typedef` and typedef-name resolution.
//! - Function pointer declarators.
//! - Designated initializers and compound literals.
//! - `switch` / `case` / `default`, `goto` / labels.
//! - Attributes other than parsing them into `attrs`.

pub mod dump;

use rccx_source::Span;

pub use dump::dump;

/// Top-level translation unit.
#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

/// A top-level declaration.
#[derive(Debug, Clone)]
pub enum Item {
    FnDef(FnDef),
    Decl(Decl),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::FnDef(f) => f.span,
            Item::Decl(d) => d.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FnDef {
    pub return_type: Type,
    pub name: Ident,
    pub params: Vec<Param>,
    pub is_variadic: bool,
    /// `None` for a prototype (no body); `Some` for a real definition.
    pub body: Option<Block>,
    pub storage: Vec<StorageSpec>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub ty: Type,
    /// `None` for unnamed parameters in prototypes.
    pub name: Option<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub ty: Type,
    pub name: Ident,
    pub init: Option<Expr>,
    pub storage: Vec<StorageSpec>,
    pub span: Span,
}

/// `static`, `extern`, `auto`, `register`, `inline`, `_Thread_local`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageSpec {
    Static,
    Extern,
    Auto,
    Register,
    Inline,
    Noreturn,
    ThreadLocal,
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

// === Types ================================================================

#[derive(Debug, Clone)]
pub struct Type {
    pub kind: TypeKind,
    pub qualifiers: TypeQualifiers,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    Void,
    Bool,
    /// One of the C builtin numeric types, after normalizing the signed /
    /// unsigned / short / long modifier combinations.
    Builtin(BuiltinType),
    Pointer(Box<Type>),
    /// `T[N]`. `size = None` represents incomplete `T[]`.
    Array {
        elem: Box<Type>,
        size: Option<Box<Expr>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinType {
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeQualifiers {
    pub is_const: bool,
    pub is_volatile: bool,
    pub is_restrict: bool,
    pub is_atomic: bool,
}

// === Statements ===========================================================

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Compound(Block),
    Expr(Expr),
    /// Local declaration of one or more variables sharing a base type.
    Decl(Vec<Decl>),
    If {
        cond: Expr,
        then_branch: Box<Stmt>,
        else_branch: Option<Box<Stmt>>,
    },
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
    DoWhile {
        body: Box<Stmt>,
        cond: Expr,
    },
    For {
        init: Option<ForInit>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    /// `;` on its own.
    Empty,
}

#[derive(Debug, Clone)]
pub enum ForInit {
    Decl(Vec<Decl>),
    Expr(Expr),
}

// === Expressions ==========================================================

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    IntLit(IntLit),
    FloatLit(FloatLit),
    CharLit(String),
    StringLit(String),
    Ident(Ident),
    Paren(Box<Expr>),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Member {
        base: Box<Expr>,
        arrow: bool,
        field: Ident,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Postfix {
        op: PostfixOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Assign {
        op: AssignOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Cast {
        ty: Type,
        expr: Box<Expr>,
    },
    SizeofExpr(Box<Expr>),
    /// Comma operator: `a, b`.
    Comma(Vec<Expr>),
}

#[derive(Debug, Clone)]
pub struct IntLit {
    /// Raw source text of the literal, including any base prefix and suffix.
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct FloatLit {
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Unary `+`.
    Plus,
    /// Unary `-`.
    Neg,
    /// `!`
    LogicalNot,
    /// `~`
    BitNot,
    /// `*` dereference.
    Deref,
    /// `&` address-of.
    AddrOf,
    /// Prefix `++`.
    PreInc,
    /// Prefix `--`.
    PreDec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostfixOp {
    Inc,
    Dec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Multiplicative.
    Mul,
    Div,
    Mod,
    // Additive.
    Add,
    Sub,
    // Shift.
    Shl,
    Shr,
    // Relational.
    Lt,
    Gt,
    LtEq,
    GtEq,
    // Equality.
    Eq,
    NotEq,
    // Bitwise.
    BitAnd,
    BitXor,
    BitOr,
    // Logical.
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`
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
