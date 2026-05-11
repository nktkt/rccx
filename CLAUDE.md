# rccx Project Rules

This repository is a Rust implementation of the C compiler `rccx`.

## Product Goal

- C17/C23 compiler with an opt-in **Safe C Mode** (`-fsafe-c`).
- Strict C mode preserves standard C semantics, including raw pointer ops.
- Safe C mode enables ownership, borrow, lifetime, and `unsafe`-boundary checks
  driven by `[[sc::owner]]`, `[[sc::borrow]]`, `[[sc::borrow_mut]]`,
  `[[sc::lifetime(...)]]`, `[[sc::nonnull]]`, `[[sc::nullable]]`,
  `[[sc::unsafe]]`.

## Pipeline (immutable)

```
source manager -> lexer -> preprocessor -> parser -> AST
              -> resolver -> HIR -> type checker
              -> MIR-C -> borrow checker -> lint
              -> codegen -> LLVM backend -> linker -> binary
```

Do not collapse AST, HIR, and MIR into one structure.

## Workspace

Each pipeline stage lives in its own crate. See `ARCHITECTURE.md` for the
canonical list. Add crates only as their phase begins.

## Engineering Rules

- Build phase by phase. Do not skip ahead.
- Add tests with every feature, not after.
- Public APIs carry doc comments explaining intent.
- Avoid `unwrap` / `expect` outside of tests and provably-unreachable cases.
- No global mutable state. Pass `SourceMap`, `DiagnosticSink`, and options
  explicitly.
- Diagnostics always carry span, code, severity, message, and (when useful)
  note + help.
- Every stage supports a debug dump where practical.
- Strict C compatibility is separate from Safe C rules.
- Do not introduce LLVM dependency until `rccx_codegen` exists.
- For non-trivial changes, post a plan first.

## Testing

- `cargo test` must pass before declaring a phase done.
- UI tests render diagnostics and compare against `tests/ui/**/*.stderr`.
- Snapshot updates are deliberate, never bulk-automated.
- CLI tests use `assert_cmd` against the built binary.

## Status

- Phase 0: in progress (workspace, source, diagnostics, driver, CLI).
- Phase 1+: not started. See `ROADMAP.md`.
