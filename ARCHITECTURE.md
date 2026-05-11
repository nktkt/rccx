# rccx Architecture

## Workspace Layout

```
crates/
  rccx_cli           # argv parsing, --version / --help / --explain, exit codes
  rccx_driver        # pipeline orchestration, options struct, query entry
  rccx_source        # FileId, SourceFile, SourceMap, span <-> line/column
  rccx_diagnostics   # Severity, DiagnosticCode, Diagnostic, renderers
  rccx_lexer         # (Phase 1)
  rccx_pp            # (Phase 2)
  rccx_parser        # (Phase 3)
  rccx_ast           # (Phase 3)
  rccx_resolve       # (Phase 4)
  rccx_hir           # (Phase 4)
  rccx_typeck        # (Phase 4)
  rccx_mir           # (Phase 5)
  rccx_codegen       # (Phase 6) backend-agnostic codegen trait
  rccx_borrowck      # (Phase 7)
  rccx_codegen_llvm  # (Phase 8)
  rccx_lint          # (Phase 9)
  rccx_query         # (Phase 9) demand-driven query DB
  rccx_incremental   # (Phase 9)
  rccx_pm            # (Phase 9) rccxpm package manager
  rccx_lsp           # (Phase 9)
  rccx_fmt           # (Phase 9)
```

Only crates listed in `Cargo.toml` exist in Phase 0. Later crates are added
phase by phase to keep the build graph small and compile times tight.

## Layering Rules

- `rccx_diagnostics` depends only on `rccx_source`.
- `rccx_source` depends on nothing project-internal.
- `rccx_driver` depends on `rccx_source` + `rccx_diagnostics` and, later, on
  each pipeline crate.
- `rccx_cli` depends on `rccx_driver` only.
- AST, HIR, and MIR are distinct crates with distinct data types. Do not
  collapse them.

## Error Handling

- Library crates return `Result<T, Diagnostic>` or push to a `DiagnosticSink`.
- `unwrap` / `expect` are reserved for tests or for invariants that cannot
  be violated without a logic bug.
- The driver collects diagnostics and renders them once at the end of a stage,
  so individual stages stay decoupled from output.

## Source Mapping

- `FileId` is an opaque `u32` index into `SourceMap`.
- A `Span` is `{ file: FileId, lo: ByteOffset, hi: ByteOffset }` (half-open).
- Line and column are computed on demand by `SourceMap::location(span)` using a
  cached vector of line-start offsets.
- Macro expansion and `#include` stacks become a second axis later (Phase 2).

## Diagnostics Rendering

A diagnostic renders as:

```
<severity>[<code>]: <message>
  --> <file>:<line>:<col>
   |
LL |     <line of source>
   |     ^^^^^ <primary label>
   = note: <note>
   = help: <help>
```

The renderer must be deterministic, fully ASCII (no Unicode arrows in Phase 0),
and stable enough for golden snapshot tests.

## Future Query System

In Phase 9 the driver becomes a thin shell around a query DB:
```
read_file(FileId)            -> Arc<SourceFile>
lex(FileId)                  -> TokenStream
preprocess(FileId)           -> PreprocessedTokens
parse(FileId)                -> AstModule
resolve(ModuleId)            -> ResolvedModule
lower_hir(ModuleId)          -> HirModule
type_check(ItemId)           -> TypeInfo
lower_mir(FunctionId)        -> MirBody
borrow_check(FunctionId)     -> BorrowReport
codegen_module(ModuleId)     -> LlvmModule
```

Until then the driver calls stages directly but keeps signatures
query-shaped so the transition is a refactor, not a rewrite.
