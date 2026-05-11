//! Token types produced by the lexer.

use rccx_source::Span;

use crate::keyword::Keyword;

/// A single lexed token: kind + source span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Base used by an integer literal, as recognized by the lexer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntBase {
    Decimal,
    Hex,
    Octal,
    /// `0b` / `0B` prefix. Standardized in C23; accepted in earlier modes for
    /// diagnostic purposes (the parser may later reject under `-std=c17`).
    Binary,
}

/// All token kinds the lexer can produce.
///
/// Whitespace, newlines, and comments are kept in the stream because the
/// preprocessor (Phase 2) needs them to detect directives and to spell out
/// macro expansions correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Trivia.
    Whitespace,
    Newline,
    LineComment,
    BlockComment,

    // Words and literals.
    Ident,
    Keyword(Keyword),
    IntLiteral {
        base: IntBase,
    },
    FloatLiteral,
    CharLiteral,
    StringLiteral,

    // Punctuators (sorted roughly by family).
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semicolon,
    Comma,
    Dot,
    Arrow,
    Ellipsis,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,

    Amp,
    Pipe,
    Caret,
    Tilde,
    Bang,

    Lt,
    Gt,
    Eq,
    LtEq,
    GtEq,
    EqEq,
    BangEq,

    AmpAmp,
    PipePipe,
    Shl,
    Shr,

    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,

    PlusPlus,
    MinusMinus,
    Question,
    Colon,

    Hash,
    HashHash,

    /// Synthetic end-of-file marker at the end of every stream.
    Eof,
    /// Bytes the lexer could not classify. Always paired with a diagnostic.
    Unknown,
}

impl TokenKind {
    /// Returns true for tokens the preprocessor and parser usually skip
    /// (whitespace, newlines, comments).
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            TokenKind::Whitespace
                | TokenKind::Newline
                | TokenKind::LineComment
                | TokenKind::BlockComment
        )
    }
}
