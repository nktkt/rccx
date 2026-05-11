//! Compiler options. Owned by the driver but populated by the CLI.

use std::path::PathBuf;
use std::str::FromStr;

/// C language standard requested via `-std=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CStandard {
    C89,
    C99,
    C11,
    #[default]
    C17,
    C23,
}

impl CStandard {
    pub fn as_str(self) -> &'static str {
        match self {
            CStandard::C89 => "c89",
            CStandard::C99 => "c99",
            CStandard::C11 => "c11",
            CStandard::C17 => "c17",
            CStandard::C23 => "c23",
        }
    }
}

impl FromStr for CStandard {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "c89" | "c90" => Ok(CStandard::C89),
            "c99" => Ok(CStandard::C99),
            "c11" => Ok(CStandard::C11),
            "c17" | "c18" => Ok(CStandard::C17),
            "c23" | "c2x" => Ok(CStandard::C23),
            other => Err(format!("unknown C standard `{other}`")),
        }
    }
}

/// Safe C mode setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SafeCMode {
    /// Standard C only; Safe C attributes are parsed but not enforced.
    #[default]
    Off,
    /// Safe C violations are errors.
    Error,
    /// Safe C violations are warnings (migration mode).
    Warn,
}

/// Intermediate form to emit via `-emit=...`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitKind {
    Tokens,
    Ast,
    Hir,
    Mir,
    LlvmIr,
    Obj,
    Asm,
}

impl FromStr for EmitKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tokens" => Ok(EmitKind::Tokens),
            "ast" => Ok(EmitKind::Ast),
            "hir" => Ok(EmitKind::Hir),
            "mir" => Ok(EmitKind::Mir),
            "llvm-ir" | "llvm" => Ok(EmitKind::LlvmIr),
            "obj" => Ok(EmitKind::Obj),
            "asm" => Ok(EmitKind::Asm),
            other => Err(format!("unknown -emit target `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub inputs: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub standard: CStandard,
    pub safe_c: SafeCMode,
    pub emit: Option<EmitKind>,
    pub json_diagnostics: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_parses_aliases() {
        assert_eq!("c90".parse::<CStandard>().unwrap(), CStandard::C89);
        assert_eq!("c18".parse::<CStandard>().unwrap(), CStandard::C17);
        assert_eq!("c2x".parse::<CStandard>().unwrap(), CStandard::C23);
    }

    #[test]
    fn standard_rejects_unknown() {
        assert!("c42".parse::<CStandard>().is_err());
    }

    #[test]
    fn emit_kind_parses() {
        assert_eq!("tokens".parse::<EmitKind>().unwrap(), EmitKind::Tokens);
        assert_eq!("llvm-ir".parse::<EmitKind>().unwrap(), EmitKind::LlvmIr);
        assert!("nonsense".parse::<EmitKind>().is_err());
    }
}
