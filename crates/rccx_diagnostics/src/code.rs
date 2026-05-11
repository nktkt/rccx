//! Stable diagnostic codes.
//!
//! Codes look like `E0001` (errors) or `W0001` (warnings) and are stable
//! across releases. `--explain <code>` prints the long-form explanation.

use std::fmt;

/// A stable diagnostic code. Currently a thin wrapper around `&'static str`
/// so codes can sit in `const` items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticCode(pub &'static str);

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Long-form text shown by `--explain <code>`. Stored as static strings so
/// the registry has zero runtime cost.
#[derive(Debug, Clone, Copy)]
pub struct ExplainEntry {
    pub code: DiagnosticCode,
    pub title: &'static str,
    pub body: &'static str,
}

// === Reserved codes ========================================================
// Codes are reserved up-front so phases can refer to them without churn.
// Phase 0 only renders one of these (E9999 / W9999 for the placeholder
// pipeline), but the long-form text exists from day one.

pub const E_USE_AFTER_MOVE: DiagnosticCode = DiagnosticCode("E0001");
pub const E_DOUBLE_FREE: DiagnosticCode = DiagnosticCode("E0002");
pub const E_DANGLING_BORROW: DiagnosticCode = DiagnosticCode("E0003");
pub const E_MUT_BORROW_CONFLICT: DiagnosticCode = DiagnosticCode("E0004");
pub const E_UNSAFE_OP_OUTSIDE_UNSAFE: DiagnosticCode = DiagnosticCode("E0005");
pub const E_RETURN_LOCAL_POINTER: DiagnosticCode = DiagnosticCode("E0006");
pub const E_USE_UNINITIALIZED: DiagnosticCode = DiagnosticCode("E0007");

pub const E_IO_FAILED: DiagnosticCode = DiagnosticCode("E9001");
pub const E_BAD_CLI: DiagnosticCode = DiagnosticCode("E9002");
pub const E_NO_INPUT: DiagnosticCode = DiagnosticCode("E9003");
pub const E_UNIMPLEMENTED: DiagnosticCode = DiagnosticCode("E9999");

const ENTRIES: &[ExplainEntry] = &[
    ExplainEntry {
        code: E_USE_AFTER_MOVE,
        title: "use of moved owner pointer",
        body: "\
A value annotated with [[sc::owner]] was passed to a function (or assigned)
that took ownership, and then used again afterwards.

Either pass the value as a borrow ([[sc::borrow]] or [[sc::borrow_mut]]) so
the caller keeps ownership, or stop using the value after the move.",
    },
    ExplainEntry {
        code: E_DOUBLE_FREE,
        title: "double free of owner pointer",
        body: "\
The same [[sc::owner]] pointer was freed (or moved into a consuming function)
more than once. Each owner has linear ownership and must be released exactly
once on every control-flow path.",
    },
    ExplainEntry {
        code: E_DANGLING_BORROW,
        title: "dangling borrow",
        body: "\
A borrow ([[sc::borrow]] or [[sc::borrow_mut]]) outlives the data it points
to. This happens when returning a pointer to a local variable, or when the
owner is freed while a borrow is still live.",
    },
    ExplainEntry {
        code: E_MUT_BORROW_CONFLICT,
        title: "conflicting borrow",
        body: "\
Safe C requires that at any program point either many [[sc::borrow]] views
exist, or a single [[sc::borrow_mut]] view exists. A shared borrow cannot
coexist with a mutable borrow or with mutation of the underlying value.",
    },
    ExplainEntry {
        code: E_UNSAFE_OP_OUTSIDE_UNSAFE,
        title: "unsafe operation outside unsafe block",
        body: "\
Operations that Safe C cannot prove sound (raw pointer dereference, pointer
arithmetic, integer-to-pointer cast, union reinterpretation, mutable global
access, inline assembly) must be wrapped in an `unsafe { ... }` block in
Safe C mode.",
    },
    ExplainEntry {
        code: E_RETURN_LOCAL_POINTER,
        title: "returning pointer to local variable",
        body: "\
The function returns a pointer that refers to one of its locals. After the
function returns, that storage is gone, so any caller dereference would be
undefined behaviour.",
    },
    ExplainEntry {
        code: E_USE_UNINITIALIZED,
        title: "use of uninitialized value",
        body: "\
A value is read on a control-flow path on which it has not been written.
Initialize the value at its declaration, or on every path that reaches the
read.",
    },
    ExplainEntry {
        code: E_IO_FAILED,
        title: "input/output failure",
        body: "\
The compiler could not read or write a file. The diagnostic message includes
the OS error reported by the operating system.",
    },
    ExplainEntry {
        code: E_BAD_CLI,
        title: "bad command-line invocation",
        body: "\
The command line passed to rccx could not be parsed. Run `rccx --help` for
the full option reference.",
    },
    ExplainEntry {
        code: E_NO_INPUT,
        title: "no input files",
        body: "\
rccx was invoked without any input files to compile. Pass at least one C
source file, or pass `--help` to see usage.",
    },
    ExplainEntry {
        code: E_UNIMPLEMENTED,
        title: "feature not yet implemented",
        body: "\
The compiler reached a code path that is reserved for a future phase. This
is not a bug in your program; it is a missing feature in rccx. See
ROADMAP.md for the implementation plan.",
    },
];

pub fn explain(code: &str) -> Option<&'static ExplainEntry> {
    ENTRIES.iter().find(|e| e.code.0 == code)
}

pub fn all_codes() -> impl Iterator<Item = &'static ExplainEntry> {
    ENTRIES.iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_resolve() {
        assert!(explain("E0001").is_some());
        assert!(explain("E9999").is_some());
    }

    #[test]
    fn unknown_code_returns_none() {
        assert!(explain("E1234").is_none());
        assert!(explain("nonsense").is_none());
    }

    #[test]
    fn entries_have_unique_codes() {
        let mut seen = std::collections::HashSet::new();
        for e in all_codes() {
            assert!(seen.insert(e.code.0), "duplicate code: {}", e.code.0);
        }
    }
}
