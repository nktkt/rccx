# rccx Specification

`rccx` is a C compiler written in Rust. It targets ISO C17 and C23, and offers
an opt-in **Safe C mode** that adds Rust-inspired ownership, borrow, lifetime,
and unsafe-boundary analysis on top of standard C semantics.

## 1. Modes

### Strict C Mode (default)
- Honors standard C semantics, including raw pointer arithmetic and casts.
- Safe C attributes are parsed but their checks are disabled.
- Goal: drop-in compatibility with existing C code.

### Safe C Mode (`-fsafe-c`)
- Enables ownership, borrow, lifetime, and unsafe checks.
- Requires `unsafe { ... }` for raw pointer dereference, pointer arithmetic,
  integer-pointer casts, union reinterpretation, mutable global access,
  and inline assembly.
- Safe C attributes:
  - `[[sc::owner]]`
  - `[[sc::borrow]]`
  - `[[sc::borrow_mut]]`
  - `[[sc::lifetime("a")]]`
  - `[[sc::nonnull]]` / `[[sc::nullable]]`
  - `[[sc::unsafe]]`

### Warn Mode (`-fsafe-c=warn`)
- Same checks as Safe C Mode, but violations are emitted as warnings.
- Intended for incremental migration of legacy C code.

## 2. Language Levels

`-std=` accepts: `c89`, `c99`, `c11`, `c17`, `c23`.

C23 specifics (planned, gated by `-std=c23`):
- `nullptr` / `nullptr_t`
- `_BitInt(N)`
- `typeof` / `typeof_unqual`
- `constexpr` object definitions
- New attribute syntax (`[[...]]`)

## 3. Pipeline

```
source manager
  -> lexer
  -> preprocessor
  -> parser
  -> AST
  -> name resolver
  -> HIR
  -> type checker
  -> MIR-C
  -> borrow checker (Safe C mode)
  -> lint / UB checker
  -> codegen abstraction
  -> LLVM backend
  -> object file -> linker -> binary
```

Each stage is exposed as a query in the demand-driven query system.

## 4. CLI

```
rccx [OPTIONS] <INPUTS>
```

Options (implementation rolls out across phases):
```
-std=c89|c99|c11|c17|c23
-fsafe-c
-fsafe-c=warn
-Wall / -Wextra / -Werror
-O0 / -O1 / -O2 / -O3
-g
-c / -S
-emit=tokens|ast|hir|mir|llvm-ir|obj|asm
-o <path>
-I <include_path>
-D <macro=value>
-U <macro>
-L <lib_path>
-l <library>
--target <triple>
--explain <error-code>
--json-diagnostics
--incremental / --no-incremental
```

## 5. Diagnostics

Every diagnostic carries:
- A stable error code (e.g. `E0001`).
- A severity (`error`, `warning`, `note`, `help`).
- A primary `Span` plus zero or more labeled spans.
- Optional `note` and `help` messages.
- The include / macro expansion stack when relevant.

Rendering targets:
- Human renderer (default).
- JSON renderer (`--json-diagnostics`).

## 6. Safe C Semantics (summary)

| Pointer kind   | Behavior                                                                 |
|----------------|--------------------------------------------------------------------------|
| `[[sc::owner]]` | Linear ownership. Must be freed exactly once. Move on call / assignment. |
| `[[sc::borrow]]` | Shared, read-only. Cannot coexist with `borrow_mut` or mutation.        |
| `[[sc::borrow_mut]]` | Exclusive, mutable. Forbids any other borrow or owner access.      |
| Raw pointer     | Strict C compatible. Requires `unsafe { ... }` for dangerous ops in Safe C mode. |

Borrow checker (MIR-C) tracks each place with the state machine:
```
Uninit -> Init -> { BorrowedShared(n), BorrowedMut } -> Init -> Moved | Freed | Dropped
```

## 7. MVP Definition

The MVP is complete when `rccx` can:
1. Compile and link a standard C `hello world` to a binary.
2. In Safe C mode, reject the canonical "double consume of owner" example
   with diagnostic `E0001` (use of moved owner pointer) or `E0002`
   (double free of owner pointer), as appropriate.

See `ROADMAP.md` for phase-by-phase scope.
