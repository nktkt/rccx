//! C17/C23 lexer for `rccx`.
//!
//! Phase 1 produces a flat token stream with spans. Whitespace, newlines, and
//! comments are kept as tokens because the preprocessor in Phase 2 needs them
//! (the C standard's preprocessing tokens preserve whitespace boundaries).
//!
//! Lexical errors (unterminated strings, unterminated block comments, stray
//! characters) are reported as [`rccx_diagnostics::Diagnostic`]s. The lexer
//! always returns a token for every byte it consumes, so the parser sees a
//! complete stream even when errors occur.

pub mod keyword;
pub mod token;

use rccx_diagnostics::code::{DiagnosticCode, E_UNIMPLEMENTED};
use rccx_diagnostics::{Diagnostic, Label};
use rccx_source::{FileId, SourceFile, Span};

pub use keyword::Keyword;
pub use token::{IntBase, Token, TokenKind};

/// Lex one source file.
///
/// The returned vector ends with a synthetic [`TokenKind::Eof`] whose span
/// is a zero-width range at the end of the file. The diagnostics vector is
/// empty on success.
pub fn lex(file: &SourceFile) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lexer = Lexer::new(file);
    let mut tokens = Vec::new();
    while let Some(tok) = lexer.next_token() {
        tokens.push(tok);
    }
    let eof_pos = lexer.text.len() as u32;
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(lexer.file_id, eof_pos, eof_pos),
    });
    (tokens, lexer.diagnostics)
}

// === Diagnostic codes owned by the lexer ==================================

pub const E_UNTERMINATED_STRING: DiagnosticCode = DiagnosticCode("E0101");
pub const E_UNTERMINATED_CHAR: DiagnosticCode = DiagnosticCode("E0102");
pub const E_UNTERMINATED_BLOCK_COMMENT: DiagnosticCode = DiagnosticCode("E0103");
pub const E_STRAY_CHARACTER: DiagnosticCode = DiagnosticCode("E0104");

struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: u32,
    file_id: FileId,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(file: &'a SourceFile) -> Self {
        let text = file.text();
        Self {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            file_id: file.id(),
            diagnostics: Vec::new(),
        }
    }

    fn peek(&self, offset: u32) -> Option<u8> {
        self.bytes.get((self.pos + offset) as usize).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek(0)?;
        // ASCII bytes advance by 1; multi-byte UTF-8 chars advance by their
        // full UTF-8 length so the position stays on a char boundary.
        if b < 0x80 {
            self.pos += 1;
        } else {
            let rest = &self.text[self.pos as usize..];
            let ch = rest.chars().next().expect("non-empty rest");
            self.pos += ch.len_utf8() as u32;
        }
        Some(b)
    }

    fn span(&self, lo: u32) -> Span {
        Span::new(self.file_id, lo, self.pos)
    }

    fn emit(&mut self, kind: TokenKind, lo: u32) -> Token {
        Token {
            kind,
            span: self.span(lo),
        }
    }

    fn next_token(&mut self) -> Option<Token> {
        let lo = self.pos;
        let b = self.peek(0)?;

        // Whitespace (non-newline) and newlines are separate token kinds so
        // the preprocessor can detect line boundaries without rescanning.
        if b == b'\n' {
            self.bump();
            return Some(self.emit(TokenKind::Newline, lo));
        }
        if b == b'\r' {
            self.bump();
            if self.peek(0) == Some(b'\n') {
                self.bump();
            }
            return Some(self.emit(TokenKind::Newline, lo));
        }
        if matches!(b, b' ' | b'\t' | b'\x0b' | b'\x0c') {
            while matches!(self.peek(0), Some(b' ' | b'\t' | b'\x0b' | b'\x0c')) {
                self.bump();
            }
            return Some(self.emit(TokenKind::Whitespace, lo));
        }

        // Comments.
        if b == b'/' && self.peek(1) == Some(b'/') {
            return Some(self.line_comment(lo));
        }
        if b == b'/' && self.peek(1) == Some(b'*') {
            return Some(self.block_comment(lo));
        }

        // String / char literals (with optional encoding prefix).
        if let Some(tok) = self.try_string_or_char(lo) {
            return Some(tok);
        }

        if is_ident_start(b) {
            return Some(self.ident_or_keyword(lo));
        }

        if b.is_ascii_digit() || (b == b'.' && self.peek(1).is_some_and(|c| c.is_ascii_digit())) {
            return Some(self.number(lo));
        }

        if let Some(tok) = self.punctuator(lo) {
            return Some(tok);
        }

        // Anything else is a stray character.
        self.bump();
        let span = self.span(lo);
        self.diagnostics.push(
            Diagnostic::error(
                E_STRAY_CHARACTER,
                format!("stray `{}` in program", display_byte(b)),
            )
            .with_label(Label::primary_unlabeled(span)),
        );
        Some(Token {
            kind: TokenKind::Unknown,
            span,
        })
    }

    fn line_comment(&mut self, lo: u32) -> Token {
        self.bump();
        self.bump();
        while let Some(b) = self.peek(0) {
            if b == b'\n' || b == b'\r' {
                break;
            }
            self.bump();
        }
        self.emit(TokenKind::LineComment, lo)
    }

    fn block_comment(&mut self, lo: u32) -> Token {
        self.bump();
        self.bump();
        let mut closed = false;
        while let Some(b) = self.peek(0) {
            if b == b'*' && self.peek(1) == Some(b'/') {
                self.bump();
                self.bump();
                closed = true;
                break;
            }
            self.bump();
        }
        if !closed {
            let span = self.span(lo);
            self.diagnostics.push(
                Diagnostic::error(E_UNTERMINATED_BLOCK_COMMENT, "unterminated /* comment")
                    .with_label(Label::primary(span, "comment starts here"))
                    .with_help("add `*/` to close the comment"),
            );
        }
        self.emit(TokenKind::BlockComment, lo)
    }

    /// Try to lex an encoding-prefixed string or character literal.
    ///
    /// Handles: `"..."`, `u"..."`, `u8"..."`, `U"..."`, `L"..."`,
    /// and `'...'`, `u'...'`, `U'...'`, `L'...'`.
    fn try_string_or_char(&mut self, lo: u32) -> Option<Token> {
        let b = self.peek(0)?;
        let mut probe = 0u32;
        if matches!(b, b'u' | b'U' | b'L') {
            probe = 1;
            if b == b'u' && self.peek(1) == Some(b'8') {
                probe = 2;
            }
            let after = self.peek(probe)?;
            if after != b'"' && after != b'\'' {
                return None;
            }
        } else if b != b'"' && b != b'\'' {
            return None;
        }
        for _ in 0..probe {
            self.bump();
        }
        let quote = self.peek(0)?;
        if quote == b'"' {
            Some(self.string_literal(lo))
        } else if quote == b'\'' {
            Some(self.char_literal(lo))
        } else {
            None
        }
    }

    fn string_literal(&mut self, lo: u32) -> Token {
        self.bump(); // opening "
        let mut closed = false;
        while let Some(b) = self.peek(0) {
            match b {
                b'"' => {
                    self.bump();
                    closed = true;
                    break;
                }
                b'\\' => {
                    self.bump();
                    if self.peek(0).is_some() {
                        self.bump();
                    }
                }
                b'\n' => break,
                _ => {
                    self.bump();
                }
            }
        }
        if !closed {
            let span = self.span(lo);
            self.diagnostics.push(
                Diagnostic::error(E_UNTERMINATED_STRING, "unterminated string literal")
                    .with_label(Label::primary(span, "literal starts here"))
                    .with_help("add a closing `\"`"),
            );
        }
        self.emit(TokenKind::StringLiteral, lo)
    }

    fn char_literal(&mut self, lo: u32) -> Token {
        self.bump(); // opening '
        let mut closed = false;
        while let Some(b) = self.peek(0) {
            match b {
                b'\'' => {
                    self.bump();
                    closed = true;
                    break;
                }
                b'\\' => {
                    self.bump();
                    if self.peek(0).is_some() {
                        self.bump();
                    }
                }
                b'\n' => break,
                _ => {
                    self.bump();
                }
            }
        }
        if !closed {
            let span = self.span(lo);
            self.diagnostics.push(
                Diagnostic::error(E_UNTERMINATED_CHAR, "unterminated character literal")
                    .with_label(Label::primary(span, "literal starts here"))
                    .with_help("add a closing `'`"),
            );
        }
        self.emit(TokenKind::CharLiteral, lo)
    }

    fn ident_or_keyword(&mut self, lo: u32) -> Token {
        while let Some(b) = self.peek(0) {
            if is_ident_continue(b) {
                self.bump();
            } else {
                break;
            }
        }
        let text = &self.text[lo as usize..self.pos as usize];
        if let Some(kw) = Keyword::from_ident(text) {
            self.emit(TokenKind::Keyword(kw), lo)
        } else {
            self.emit(TokenKind::Ident, lo)
        }
    }

    /// Lex a numeric literal (integer or float).
    ///
    /// Phase 1 stays close to the C "pp-number" grammar: a long run of
    /// digit-like / dot / sign-after-exponent characters. The token records
    /// whether it looks integer- or float-shaped and what base it used; the
    /// type checker re-parses the slice for the real numeric value later.
    fn number(&mut self, lo: u32) -> Token {
        let first = self.peek(0).unwrap();
        let mut is_float = false;
        let mut base = IntBase::Decimal;

        if first == b'.' {
            is_float = true;
            self.bump();
        } else if first == b'0' {
            self.bump();
            match self.peek(0) {
                Some(b'x' | b'X') => {
                    base = IntBase::Hex;
                    self.bump();
                }
                Some(b'b' | b'B') => {
                    base = IntBase::Binary;
                    self.bump();
                }
                Some(c) if c.is_ascii_digit() => {
                    base = IntBase::Octal;
                }
                _ => {}
            }
        }

        while let Some(b) = self.peek(0) {
            let valid_digit = match base {
                IntBase::Decimal | IntBase::Octal => b.is_ascii_digit() || b == b'\'',
                IntBase::Hex => b.is_ascii_hexdigit() || b == b'\'',
                IntBase::Binary => matches!(b, b'0' | b'1' | b'\''),
            };
            if valid_digit {
                self.bump();
                continue;
            }
            if b == b'.' {
                is_float = true;
                self.bump();
                continue;
            }
            // Exponent: `e`/`E` for decimal, `p`/`P` for hex floats.
            let is_exp = match base {
                IntBase::Hex => matches!(b, b'p' | b'P'),
                _ => matches!(b, b'e' | b'E'),
            };
            if is_exp {
                is_float = true;
                self.bump();
                if matches!(self.peek(0), Some(b'+' | b'-')) {
                    self.bump();
                }
                continue;
            }
            // Suffix characters and additional identifier-like continuation
            // bytes are kept as part of the literal; the typechecker validates
            // them later. This matches the liberal "pp-number" grammar.
            if is_ident_continue(b) {
                self.bump();
                continue;
            }
            break;
        }

        let kind = if is_float {
            TokenKind::FloatLiteral
        } else {
            TokenKind::IntLiteral { base }
        };
        self.emit(kind, lo)
    }

    fn punctuator(&mut self, lo: u32) -> Option<Token> {
        use TokenKind as T;
        let b0 = self.peek(0)?;
        let b1 = self.peek(1);
        let b2 = self.peek(2);

        // Order matters: longest match first.
        let (kind, len) = match (b0, b1, b2) {
            (b'.', Some(b'.'), Some(b'.')) => (T::Ellipsis, 3),
            (b'<', Some(b'<'), Some(b'=')) => (T::ShlEq, 3),
            (b'>', Some(b'>'), Some(b'=')) => (T::ShrEq, 3),
            (b'-', Some(b'>'), _) => (T::Arrow, 2),
            (b'+', Some(b'+'), _) => (T::PlusPlus, 2),
            (b'-', Some(b'-'), _) => (T::MinusMinus, 2),
            (b'<', Some(b'<'), _) => (T::Shl, 2),
            (b'>', Some(b'>'), _) => (T::Shr, 2),
            (b'<', Some(b'='), _) => (T::LtEq, 2),
            (b'>', Some(b'='), _) => (T::GtEq, 2),
            (b'=', Some(b'='), _) => (T::EqEq, 2),
            (b'!', Some(b'='), _) => (T::BangEq, 2),
            (b'&', Some(b'&'), _) => (T::AmpAmp, 2),
            (b'|', Some(b'|'), _) => (T::PipePipe, 2),
            (b'+', Some(b'='), _) => (T::PlusEq, 2),
            (b'-', Some(b'='), _) => (T::MinusEq, 2),
            (b'*', Some(b'='), _) => (T::StarEq, 2),
            (b'/', Some(b'='), _) => (T::SlashEq, 2),
            (b'%', Some(b'='), _) => (T::PercentEq, 2),
            (b'&', Some(b'='), _) => (T::AmpEq, 2),
            (b'|', Some(b'='), _) => (T::PipeEq, 2),
            (b'^', Some(b'='), _) => (T::CaretEq, 2),
            (b'#', Some(b'#'), _) => (T::HashHash, 2),
            (b'(', _, _) => (T::LParen, 1),
            (b')', _, _) => (T::RParen, 1),
            (b'{', _, _) => (T::LBrace, 1),
            (b'}', _, _) => (T::RBrace, 1),
            (b'[', _, _) => (T::LBracket, 1),
            (b']', _, _) => (T::RBracket, 1),
            (b';', _, _) => (T::Semicolon, 1),
            (b',', _, _) => (T::Comma, 1),
            (b'.', _, _) => (T::Dot, 1),
            (b'+', _, _) => (T::Plus, 1),
            (b'-', _, _) => (T::Minus, 1),
            (b'*', _, _) => (T::Star, 1),
            (b'/', _, _) => (T::Slash, 1),
            (b'%', _, _) => (T::Percent, 1),
            (b'&', _, _) => (T::Amp, 1),
            (b'|', _, _) => (T::Pipe, 1),
            (b'^', _, _) => (T::Caret, 1),
            (b'~', _, _) => (T::Tilde, 1),
            (b'!', _, _) => (T::Bang, 1),
            (b'<', _, _) => (T::Lt, 1),
            (b'>', _, _) => (T::Gt, 1),
            (b'=', _, _) => (T::Eq, 1),
            (b'?', _, _) => (T::Question, 1),
            (b':', _, _) => (T::Colon, 1),
            (b'#', _, _) => (T::Hash, 1),
            _ => return None,
        };
        for _ in 0..len {
            self.bump();
        }
        Some(self.emit(kind, lo))
    }
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_continue(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

fn display_byte(b: u8) -> String {
    if (0x20..0x7f).contains(&b) {
        (b as char).to_string()
    } else {
        format!("\\x{b:02x}")
    }
}

// Silence unused-import warning for the placeholder code reservation above.
const _: DiagnosticCode = E_UNIMPLEMENTED;

#[cfg(test)]
mod tests {
    use super::*;
    use rccx_source::SourceMap;

    fn lex_str(text: &str) -> (Vec<Token>, Vec<Diagnostic>) {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", text);
        let file = sm.file(id).unwrap();
        lex(file)
    }

    fn kinds(text: &str) -> Vec<TokenKind> {
        lex_str(text).0.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn empty_input_yields_only_eof() {
        let (toks, diags) = lex_str("");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::Eof);
        assert!(diags.is_empty());
    }

    #[test]
    fn whitespace_and_newline_are_distinct() {
        let ks = kinds("  \n\t");
        assert_eq!(
            ks,
            vec![
                TokenKind::Whitespace,
                TokenKind::Newline,
                TokenKind::Whitespace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn crlf_is_one_newline_token() {
        let ks = kinds("a\r\nb");
        assert_eq!(
            ks,
            vec![
                TokenKind::Ident,
                TokenKind::Newline,
                TokenKind::Ident,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keywords_and_idents() {
        let ks = kinds("int main x_1 return constexpr");
        assert_eq!(
            ks,
            vec![
                TokenKind::Keyword(Keyword::Int),
                TokenKind::Whitespace,
                TokenKind::Ident,
                TokenKind::Whitespace,
                TokenKind::Ident,
                TokenKind::Whitespace,
                TokenKind::Keyword(Keyword::Return),
                TokenKind::Whitespace,
                TokenKind::Keyword(Keyword::Constexpr),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn integer_literals_detect_base() {
        let (toks, diags) = lex_str("0 42 0x1f 0b1011 0755 12'345");
        assert!(diags.is_empty());
        let bases: Vec<_> = toks
            .iter()
            .filter_map(|t| match t.kind {
                TokenKind::IntLiteral { base } => Some(base),
                _ => None,
            })
            .collect();
        assert_eq!(
            bases,
            vec![
                IntBase::Decimal,
                IntBase::Decimal,
                IntBase::Hex,
                IntBase::Binary,
                IntBase::Octal,
                IntBase::Decimal,
            ]
        );
    }

    #[test]
    fn integer_suffix_is_part_of_literal() {
        let (toks, _) = lex_str("42ULL");
        let lit = &toks[0];
        assert!(matches!(lit.kind, TokenKind::IntLiteral { .. }));
        assert_eq!(lit.span.lo, 0);
        assert_eq!(lit.span.hi, 5);
    }

    #[test]
    fn float_literal_with_exponent() {
        let (toks, _) = lex_str("1.5e+10");
        assert!(matches!(toks[0].kind, TokenKind::FloatLiteral));
        assert_eq!(toks[0].span.hi, 7);
    }

    #[test]
    fn dot_then_digit_is_float() {
        let (toks, _) = lex_str(".25f");
        assert!(matches!(toks[0].kind, TokenKind::FloatLiteral));
    }

    #[test]
    fn hex_float() {
        let (toks, _) = lex_str("0x1.8p3");
        assert!(matches!(toks[0].kind, TokenKind::FloatLiteral));
    }

    #[test]
    fn string_literal_with_escape() {
        let (toks, diags) = lex_str(r#""hello\n\"world""#);
        assert!(diags.is_empty());
        assert!(matches!(toks[0].kind, TokenKind::StringLiteral));
    }

    #[test]
    fn unterminated_string_is_diagnosed() {
        let (toks, diags) = lex_str("\"oops\n");
        assert!(matches!(toks[0].kind, TokenKind::StringLiteral));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0101");
    }

    #[test]
    fn unterminated_block_comment_is_diagnosed() {
        let (toks, diags) = lex_str("/* never ends");
        assert!(matches!(toks[0].kind, TokenKind::BlockComment));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0103");
    }

    #[test]
    fn prefixed_string_literals() {
        for prefix in ["u", "u8", "U", "L"] {
            let s = format!("{prefix}\"x\"");
            let (toks, diags) = lex_str(&s);
            assert!(diags.is_empty(), "prefix={prefix}");
            assert!(
                matches!(toks[0].kind, TokenKind::StringLiteral),
                "prefix={prefix}, kinds={:?}",
                toks.iter().map(|t| t.kind).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn char_literal_basic() {
        let (toks, diags) = lex_str("'a'");
        assert!(diags.is_empty());
        assert!(matches!(toks[0].kind, TokenKind::CharLiteral));
    }

    #[test]
    fn unterminated_char_is_diagnosed() {
        let (_, diags) = lex_str("'oops\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0102");
    }

    #[test]
    fn punctuators_longest_match() {
        let ks = kinds("...<<=>>=->++ -- == != <= >= && ||");
        let punct: Vec<_> = ks
            .into_iter()
            .filter(|k| !matches!(k, TokenKind::Whitespace | TokenKind::Eof))
            .collect();
        assert_eq!(
            punct,
            vec![
                TokenKind::Ellipsis,
                TokenKind::ShlEq,
                TokenKind::ShrEq,
                TokenKind::Arrow,
                TokenKind::PlusPlus,
                TokenKind::MinusMinus,
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
            ]
        );
    }

    #[test]
    fn line_comment_ends_at_newline() {
        let (toks, _) = lex_str("a // tail\nb");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident,
                TokenKind::Whitespace,
                TokenKind::LineComment,
                TokenKind::Newline,
                TokenKind::Ident,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn block_comment_spans_lines() {
        let (toks, diags) = lex_str("/* a\nb */c");
        assert!(diags.is_empty());
        assert!(matches!(toks[0].kind, TokenKind::BlockComment));
        // "/* a\nb */" is 9 bytes; the comment span runs [0, 9).
        assert_eq!(toks[0].span.hi, 9);
    }

    #[test]
    fn stray_character_is_diagnosed() {
        let (toks, diags) = lex_str("@");
        assert_eq!(toks[0].kind, TokenKind::Unknown);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0104");
    }

    #[test]
    fn unicode_after_token_is_stray_but_does_not_panic() {
        let (toks, diags) = lex_str("int あ");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TokenKind::Unknown));
        assert!(!diags.is_empty());
    }

    #[test]
    fn spans_round_trip_through_source() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", "int x = 42;");
        let file = sm.file(id).unwrap();
        let (toks, _) = lex(file);
        let ident = toks
            .iter()
            .find(|t| t.kind == TokenKind::Ident)
            .expect("ident exists");
        assert_eq!(file.slice(ident.span), Some("x"));
    }
}
