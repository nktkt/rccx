# rccx

A C compiler written in Rust, with an opt-in **Safe C mode** that adds
Rust-inspired ownership, borrow, lifetime, and `unsafe`-boundary analysis on
top of standard ISO C.

> **Status:** Phase 0. The CLI, source manager, and diagnostics are wired
> end-to-end. The lexer, parser, type checker, MIR, borrow checker, and
> codegen are scheduled in later phases. See [`ROADMAP.md`](ROADMAP.md).

## Why another C compiler?

Most safety work in C lives outside the language: sanitizers, static
analyzers, and coding guidelines. `rccx` takes a different bet — keep the
language standard exactly as it is, but offer a *second* mode where extra
attributes turn on Rust-style compile-time checks:

| Pointer kind            | Behavior in Safe C mode                                                  |
|-------------------------|--------------------------------------------------------------------------|
| `[[sc::owner]] T*`      | Linear ownership. Must be freed exactly once. Moved on assignment / call.|
| `[[sc::borrow]] T*`     | Shared, read-only view. Forbids `borrow_mut` and mutation while live.    |
| `[[sc::borrow_mut]] T*` | Exclusive, mutable view. Forbids any other borrow or owner access.       |
| Raw `T*`                | Standard C semantics. Dangerous ops require `unsafe { ... }`.            |

A program that compiles cleanly under `-fsafe-c` is statically free of
use-after-free, double-free, dangling borrow, mutable-aliasing, and
uninitialized-read bugs — without changing the C standard.

## Modes

```bash
rccx main.c                  # Strict C mode (standard C semantics)
rccx main.c -fsafe-c         # Safe C mode (violations are errors)
rccx main.c -fsafe-c=warn    # Migration mode (violations are warnings)
```

## Example (planned diagnostic, Phase 7)

```c
// main.c
#include <stdlib.h>

void consume([[sc::owner]] int *p) { free(p); }

int main(void) {
    [[sc::owner]] int *p = malloc(sizeof(int));
    consume(p);
    consume(p);  // use of moved owner pointer
}
```

```text
error[E0001]: use of moved owner pointer
  --> main.c:8:13
  |
6 |     [[sc::owner]] int *p = malloc(sizeof(int));
  |                        - owner allocated here
7 |     consume(p);
  |             - ownership moved here
8 |     consume(p);
  |             ^ value used after move
  = help: pass a borrow ([[sc::borrow]] or [[sc::borrow_mut]]) instead
```

## Pipeline

```
source manager -> lexer -> preprocessor -> parser -> AST
              -> resolver -> HIR -> type checker
              -> MIR-C -> borrow checker -> lint
              -> codegen -> LLVM backend -> linker -> binary
```

Each stage lives in its own crate under [`crates/`](crates/). AST, HIR, and
MIR are kept as distinct data structures so the borrow checker can reason on
a simple, control-flow-oriented IR rather than on raw C syntax.

## Quick start

Requires a recent stable Rust toolchain (see [`rust-toolchain.toml`](rust-toolchain.toml)).

```bash
cargo build
cargo test

cargo run -p rccx_cli -- --version
cargo run -p rccx_cli -- --help
cargo run -p rccx_cli -- --explain E0001
```

## Workspace layout

| Crate              | Phase | Purpose                                                    |
|--------------------|-------|------------------------------------------------------------|
| `rccx_source`      | 0     | `FileId`, `Span`, `SourceMap`, line/column resolution.     |
| `rccx_diagnostics` | 0     | `Severity`, codes, labels, human renderer, `--explain`.    |
| `rccx_driver`      | 0     | Options, pipeline orchestration, top-level `compile()`.    |
| `rccx_cli`         | 0     | `rccx` binary entry point.                                 |
| `rccx_lexer`       | 1     | C17/C23 lexer.                                             |
| `rccx_pp`          | 2     | Preprocessor (`#include`, macros, conditionals).           |
| `rccx_parser`      | 3     | C parser producing an AST.                                 |
| `rccx_resolve`     | 4     | Name resolution, scopes, namespaces.                       |
| `rccx_typeck`      | 4     | Type checking; produces a typed HIR.                       |
| `rccx_mir`         | 5     | MIR-C (control flow, locals, moves, borrows).              |
| `rccx_codegen`     | 6     | Backend-agnostic codegen trait.                            |
| `rccx_borrowck`    | 7     | Safe C ownership / borrow / lifetime checker.              |
| `rccx_codegen_llvm`| 8     | LLVM IR + object files.                                    |
| `rccx_pm`          | 9     | `rccxpm` package manager (Cargo-style).                    |
| `rccx_lsp`         | 9     | Language server.                                           |

Phase 0 ships the first four crates; later crates land with their phases.

## Roadmap (short version)

- **Phase 0 — Foundations.** Workspace, CLI, source manager, diagnostics. ✅
- **Phase 1 — Lexer.** C17/C23 tokens, `-emit=tokens`.
- **Phase 2 — Preprocessor.** `#include`, macros, conditional compilation.
- **Phase 3 — Parser + AST.**
- **Phase 4 — Resolve + Type Check + HIR.**
- **Phase 5 — MIR-C.**
- **Phase 6 — Minimal codegen.** Compile + link `hello.c`.
- **Phase 7 — Safe C borrow checker.**
- **Phase 8 — LLVM backend.**
- **Phase 9 — Tooling.** `rccxpm`, LSP, formatter, incremental compilation.

Full plan in [`ROADMAP.md`](ROADMAP.md). Specification in
[`SPEC.md`](SPEC.md). Architecture in [`ARCHITECTURE.md`](ARCHITECTURE.md).
Engineering rules in [`CLAUDE.md`](CLAUDE.md).

## Diagnostic codes

`--explain <CODE>` prints the long-form text. Codes reserved so far:

| Code   | Meaning                                       |
|--------|-----------------------------------------------|
| E0001  | Use of moved owner pointer.                   |
| E0002  | Double free of owner pointer.                 |
| E0003  | Dangling borrow.                              |
| E0004  | Conflicting borrow.                           |
| E0005  | Unsafe operation outside `unsafe` block.      |
| E0006  | Returning pointer to local variable.          |
| E0007  | Use of uninitialized value.                   |
| E9001  | I/O failure.                                  |
| E9002  | Bad command-line invocation.                  |
| E9003  | No input files.                               |
| E9999  | Feature not yet implemented.                  |

## Contributing

The project is in its earliest phases, so issues and discussion are more
useful than PRs right now. The engineering rules in [`CLAUDE.md`](CLAUDE.md)
apply to humans too:

- Build phase by phase. Don't skip ahead.
- Tests land with features, not after.
- No `unwrap` / `expect` outside tests and provably-unreachable cases.
- Diagnostics always carry span, code, severity, message, and (when useful)
  note + help.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.
