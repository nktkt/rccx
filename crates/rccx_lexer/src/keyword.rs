//! C17 and C23 keywords.
//!
//! Every keyword recognized in any supported language level lives in this
//! enum. The parser later decides whether a given keyword is valid for the
//! `-std=` selected by the user (e.g. `constexpr` is C23-only).

/// All C keywords across the standards `rccx` understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    // C89 / C99 / C11 / C17 reserved words.
    Auto,
    Break,
    Case,
    Char,
    Const,
    Continue,
    Default,
    Do,
    Double,
    Else,
    Enum,
    Extern,
    Float,
    For,
    Goto,
    If,
    Inline,
    Int,
    Long,
    Register,
    Restrict,
    Return,
    Short,
    Signed,
    Sizeof,
    Static,
    Struct,
    Switch,
    Typedef,
    Union,
    Unsigned,
    Void,
    Volatile,
    While,

    // C11 underscore-prefixed keywords.
    Alignas_,
    Alignof_,
    Atomic_,
    Bool_,
    Complex_,
    Generic_,
    Imaginary_,
    Noreturn_,
    StaticAssert_,
    ThreadLocal_,

    // C23 additions / unprefixed aliases.
    Alignas,
    Alignof,
    Bool,
    Constexpr,
    False,
    Nullptr,
    StaticAssert,
    ThreadLocal,
    True,
    Typeof,
    TypeofUnqual,
    BitInt_,
    Decimal32_,
    Decimal64_,
    Decimal128_,
}

impl Keyword {
    /// Map an identifier spelling to a keyword, or `None` if it's an
    /// ordinary identifier.
    pub fn from_ident(s: &str) -> Option<Keyword> {
        Some(match s {
            // C17 core.
            "auto" => Keyword::Auto,
            "break" => Keyword::Break,
            "case" => Keyword::Case,
            "char" => Keyword::Char,
            "const" => Keyword::Const,
            "continue" => Keyword::Continue,
            "default" => Keyword::Default,
            "do" => Keyword::Do,
            "double" => Keyword::Double,
            "else" => Keyword::Else,
            "enum" => Keyword::Enum,
            "extern" => Keyword::Extern,
            "float" => Keyword::Float,
            "for" => Keyword::For,
            "goto" => Keyword::Goto,
            "if" => Keyword::If,
            "inline" => Keyword::Inline,
            "int" => Keyword::Int,
            "long" => Keyword::Long,
            "register" => Keyword::Register,
            "restrict" => Keyword::Restrict,
            "return" => Keyword::Return,
            "short" => Keyword::Short,
            "signed" => Keyword::Signed,
            "sizeof" => Keyword::Sizeof,
            "static" => Keyword::Static,
            "struct" => Keyword::Struct,
            "switch" => Keyword::Switch,
            "typedef" => Keyword::Typedef,
            "union" => Keyword::Union,
            "unsigned" => Keyword::Unsigned,
            "void" => Keyword::Void,
            "volatile" => Keyword::Volatile,
            "while" => Keyword::While,

            // C11 underscore forms.
            "_Alignas" => Keyword::Alignas_,
            "_Alignof" => Keyword::Alignof_,
            "_Atomic" => Keyword::Atomic_,
            "_Bool" => Keyword::Bool_,
            "_Complex" => Keyword::Complex_,
            "_Generic" => Keyword::Generic_,
            "_Imaginary" => Keyword::Imaginary_,
            "_Noreturn" => Keyword::Noreturn_,
            "_Static_assert" => Keyword::StaticAssert_,
            "_Thread_local" => Keyword::ThreadLocal_,

            // C23.
            "alignas" => Keyword::Alignas,
            "alignof" => Keyword::Alignof,
            "bool" => Keyword::Bool,
            "constexpr" => Keyword::Constexpr,
            "false" => Keyword::False,
            "nullptr" => Keyword::Nullptr,
            "static_assert" => Keyword::StaticAssert,
            "thread_local" => Keyword::ThreadLocal,
            "true" => Keyword::True,
            "typeof" => Keyword::Typeof,
            "typeof_unqual" => Keyword::TypeofUnqual,
            "_BitInt" => Keyword::BitInt_,
            "_Decimal32" => Keyword::Decimal32_,
            "_Decimal64" => Keyword::Decimal64_,
            "_Decimal128" => Keyword::Decimal128_,

            _ => return None,
        })
    }

    /// Source spelling of the keyword.
    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::Auto => "auto",
            Keyword::Break => "break",
            Keyword::Case => "case",
            Keyword::Char => "char",
            Keyword::Const => "const",
            Keyword::Continue => "continue",
            Keyword::Default => "default",
            Keyword::Do => "do",
            Keyword::Double => "double",
            Keyword::Else => "else",
            Keyword::Enum => "enum",
            Keyword::Extern => "extern",
            Keyword::Float => "float",
            Keyword::For => "for",
            Keyword::Goto => "goto",
            Keyword::If => "if",
            Keyword::Inline => "inline",
            Keyword::Int => "int",
            Keyword::Long => "long",
            Keyword::Register => "register",
            Keyword::Restrict => "restrict",
            Keyword::Return => "return",
            Keyword::Short => "short",
            Keyword::Signed => "signed",
            Keyword::Sizeof => "sizeof",
            Keyword::Static => "static",
            Keyword::Struct => "struct",
            Keyword::Switch => "switch",
            Keyword::Typedef => "typedef",
            Keyword::Union => "union",
            Keyword::Unsigned => "unsigned",
            Keyword::Void => "void",
            Keyword::Volatile => "volatile",
            Keyword::While => "while",
            Keyword::Alignas_ => "_Alignas",
            Keyword::Alignof_ => "_Alignof",
            Keyword::Atomic_ => "_Atomic",
            Keyword::Bool_ => "_Bool",
            Keyword::Complex_ => "_Complex",
            Keyword::Generic_ => "_Generic",
            Keyword::Imaginary_ => "_Imaginary",
            Keyword::Noreturn_ => "_Noreturn",
            Keyword::StaticAssert_ => "_Static_assert",
            Keyword::ThreadLocal_ => "_Thread_local",
            Keyword::Alignas => "alignas",
            Keyword::Alignof => "alignof",
            Keyword::Bool => "bool",
            Keyword::Constexpr => "constexpr",
            Keyword::False => "false",
            Keyword::Nullptr => "nullptr",
            Keyword::StaticAssert => "static_assert",
            Keyword::ThreadLocal => "thread_local",
            Keyword::True => "true",
            Keyword::Typeof => "typeof",
            Keyword::TypeofUnqual => "typeof_unqual",
            Keyword::BitInt_ => "_BitInt",
            Keyword::Decimal32_ => "_Decimal32",
            Keyword::Decimal64_ => "_Decimal64",
            Keyword::Decimal128_ => "_Decimal128",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_known_keywords() {
        for spelling in [
            "int",
            "return",
            "_Static_assert",
            "constexpr",
            "nullptr",
            "_BitInt",
        ] {
            let kw = Keyword::from_ident(spelling).expect(spelling);
            assert_eq!(kw.as_str(), spelling);
        }
    }

    #[test]
    fn non_keyword_returns_none() {
        assert!(Keyword::from_ident("Int").is_none());
        assert!(Keyword::from_ident("constepxr").is_none());
        assert!(Keyword::from_ident("").is_none());
    }
}
