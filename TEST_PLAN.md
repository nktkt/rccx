# rccx Test Plan

## Test Categories

1. **Unit tests** — per-crate, colocated in `src/` with `#[cfg(test)]`.
2. **Golden / UI tests** — diagnostic output stability. Stored under
   `tests/ui/<area>/`.
3. **CLI smoke tests** — exercise the compiled `rccx` binary. Live in
   `crates/rccx_cli/tests/`.
4. **Integration tests** — multi-stage; added from Phase 3 onward.
5. **Differential tests** — compare against the system C compiler when
   practical. Added from Phase 6 onward.

## Phase 0 Coverage Targets

| Area              | Required tests                                                |
|-------------------|---------------------------------------------------------------|
| `rccx_source`     | Empty file, single line, multi-line, last-line-no-newline,    |
|                   | byte offset -> (line, col), span -> line slice.               |
| `rccx_diagnostics`| Renderer formatting for error/warning/note/help, multi-label, |
|                   | code-only diagnostic, missing file rendering.                 |
| `rccx_cli`        | `--version` exits 0 with version string,                      |
|                   | `--help` exits 0 with usage,                                  |
|                   | `--explain E0001` prints placeholder explanation,             |
|                   | unknown flag exits non-zero with helpful diagnostic,          |
|                   | missing input file produces a real diagnostic.                |

## Conventions

- Snapshot tests compare against text files in `tests/ui/**/*.stderr`.
- Updating snapshots is a deliberate act, not automated.
- No test uses real network or external compilers in Phase 0.
- All tests run on `cargo test` with no extra setup.

## Future Phases (sketch)

- Phase 1: lexer property tests for round-tripping spans of every token kind.
- Phase 2: preprocessor expansion fixtures with golden output.
- Phase 3: parser AST-dump golden tests + error recovery cases.
- Phase 4: type checker diagnostic fixtures.
- Phase 7: Safe C borrowck UI tests, one fixture per error class.
- Phase 8: end-to-end "compile + run + check exit code" tests.
