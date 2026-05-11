# rccx Roadmap

Phases land in order. Each phase ends with `cargo test` green and a concrete
demo artifact.

## Phase 0 — Foundations (this phase)
- Workspace and tooling files.
- `rccx_source`, `rccx_diagnostics`, `rccx_driver`, `rccx_cli`.
- `rccx --version`, `rccx --help`, `rccx --explain <code>`.
- Diagnostic renderer with golden tests.
- CLI smoke tests via `assert_cmd`.

Demo: `cargo run -p rccx_cli -- --version` prints the version. `cargo test`
passes.

## Phase 1 — Lexer
- `rccx_lexer` token kinds covering C17/C23 lexemes.
- Span-preserving tokens, `-emit=tokens` in the CLI.
- Lex error diagnostics for unterminated strings, bad escapes, etc.

Demo: `rccx -emit=tokens examples/hello.c` dumps tokens.

## Phase 2 — Preprocessor
- `rccx_pp` with `#include`, `#define`, object- and function-like macros,
  conditional compilation, `#`, `##`, `__FILE__`, `__LINE__`.
- Include-stack diagnostics.

Demo: `rccx -E hello.c` prints preprocessed text.

## Phase 3 — Parser + AST
- `rccx_ast`, `rccx_parser`.
- Declarations, expressions, statements, struct/union/enum/typedef.
- Error recovery; `-emit=ast`.

Demo: AST dump for non-trivial fixtures stays stable across runs.

## Phase 4 — Resolve + Type Check + HIR
- `rccx_resolve`, `rccx_hir`, `rccx_typeck`.
- Scopes, namespaces, typedef-name resolution, implicit conversions.

## Phase 5 — MIR-C
- `rccx_mir` with blocks, locals, places, rvalues, statements, terminators.
- `Move`, `Borrow`, `Free`, `Drop`, `Assign`, `Call`.
- `-emit=mir`.

## Phase 6 — Minimal Codegen
- `rccx_codegen` trait, plus a temporary backend (C source or tiny IR) good
  enough to lower hello-world.
- Integration: compile and link a hello-world end to end.

## Phase 7 — Safe C Borrow Checker
- `rccx_borrowck` with state machine
  `Uninit | Init | Moved | Freed | BorrowedShared(n) | BorrowedMut | Dropped`.
- Use-after-move, double-free, dangling borrow, exclusivity, unsafe boundary.
- Rust-style multi-span diagnostics.

## Phase 8 — LLVM Backend
- `rccx_codegen_llvm` via `inkwell` or `llvm-sys`.
- Real object files + linker invocation.
- Debug info MVP.

## Phase 9 — Tooling
- `rccx_pm` (`rccxpm`), `rccx_lsp`, `rccx_fmt`, `rccx_lint`,
  `rccx_query`, `rccx_incremental`.
- `rccxpm new / build / run / test / fmt / lint / doc / clean`.
- LSP server with completion, diagnostics, hover.

## Done Criteria
The project hits "MVP" when both of these run:

1. Compile + run a standard C `hello.c`.
2. `rccx main.c -fsafe-c` rejects the canonical "double consume of owner"
   example with the right diagnostic code and a Rust-style multi-span report.
