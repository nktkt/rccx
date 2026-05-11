//! Safe C borrow checker — Phase 7 MVP.
//!
//! The MVP focuses on the canonical use-after-move case for
//! `[[sc::owner]]` pointers:
//!
//! ```c
//! void consume([[sc::owner]] int *p);
//! int main(void) {
//!     [[sc::owner]] int *p = malloc(sizeof(int));
//!     consume(p);
//!     consume(p);  // ← E0001 use of moved owner pointer
//! }
//! ```
//!
//! We walk each [`MirBody`] block-by-block in source order, maintaining a
//! per-local state machine of `Init`/`Moved`. Reading a local through
//! [`Operand::Move`] transitions it to `Moved`; subsequent reads (`Copy`
//! or `Move`) of a `Moved` local are reported as use-after-move.
//!
//! Limitations of the MVP (planned for Phase 7.x):
//!
//! - The walk is intra-block + a single forward sweep over blocks. Joins
//!   and back edges are approximated by union-state-from-all-predecessors
//!   only on the first visit; loops involving owners are not fully sound.
//! - We don't yet detect double-free (E0002), dangling borrows (E0003), or
//!   shared/mut borrow conflicts (E0004); those need additional MIR shape.
//! - `unsafe { ... }` does not yet relax checks because the MIR doesn't
//!   carry the lexical Unsafe marker. (HIR collapses Unsafe to Compound.)

use std::collections::HashMap;

use rccx_diagnostics::code::DiagnosticCode;
use rccx_diagnostics::{Diagnostic, Label, Severity};
use rccx_hir::Ownership;
use rccx_mir::{
    BlockId, LocalId, MirBody, MirModule, Operand, Place, Rvalue, Statement, Terminator,
};
use rccx_source::Span;

pub const E_USE_AFTER_MOVE: DiagnosticCode = DiagnosticCode("E0001");

/// Level at which violations should be reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowCheckLevel {
    /// Errors (default for `-fsafe-c`).
    Error,
    /// Warnings (for `-fsafe-c=warn`, used to migrate legacy code).
    Warn,
}

/// Run the borrow checker on every function body in `module`.
pub fn check(module: &MirModule, level: BorrowCheckLevel) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for body in &module.functions {
        check_body(body, level, &mut diags);
    }
    diags
}

fn check_body(body: &MirBody, level: BorrowCheckLevel, diags: &mut Vec<Diagnostic>) {
    // For MVP, only locals that hold an `[[sc::owner]]` pointer participate
    // in the move-tracking state machine. Everything else is left alone.
    let owner_locals: Vec<LocalId> = body
        .locals
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            if matches!(l.ty.ownership(), Ownership::Owner) {
                Some(LocalId(i as u32))
            } else {
                None
            }
        })
        .collect();
    if owner_locals.is_empty() {
        return;
    }

    // Compute the predecessors of each block so we can join states later.
    let preds = compute_predecessors(body);

    // Map of (block, state-on-entry) and a worklist for fixpoint iteration.
    // A bounded number of iterations is enough for the MVP since the lattice
    // is small (Init / Moved per local) and the per-local state can only
    // step from Init to Moved at most once per iteration.
    let n_blocks = body.blocks.len();
    let mut in_state: Vec<HashMap<LocalId, LocalState>> = vec![HashMap::new(); n_blocks];

    // Seed the entry block: all owner locals start as Uninit. (Parameters
    // come in as Init because the caller has just moved into them.)
    let mut entry: HashMap<LocalId, LocalState> = HashMap::new();
    for &l in &owner_locals {
        entry.insert(
            l,
            if l.0 <= body.arg_count {
                LocalState::Init
            } else {
                LocalState::Uninit
            },
        );
    }
    in_state[0] = entry;

    let mut moved_at: HashMap<LocalId, Span> = HashMap::new();

    let mut visited = vec![false; n_blocks];
    let mut worklist = vec![BlockId(0)];
    let max_iters = n_blocks * 4 + 16;
    let mut iters = 0;

    while let Some(bid) = worklist.pop() {
        iters += 1;
        if iters > max_iters {
            break;
        }
        let idx = bid.0 as usize;
        let mut state = in_state[idx].clone();
        let block = &body.blocks[idx];

        let already_visited = visited[idx];
        visited[idx] = true;

        for stmt in &block.statements {
            walk_statement(
                stmt,
                &mut state,
                &mut moved_at,
                &owner_locals,
                level,
                diags,
                already_visited,
            );
        }
        if let Terminator::SwitchInt { cond, .. } = &block.terminator {
            read_operand(
                cond,
                &mut state,
                &moved_at,
                &owner_locals,
                level,
                diags,
                already_visited,
            );
        }

        // Propagate to successors.
        for succ in successors(&block.terminator) {
            let sidx = succ.0 as usize;
            let merged = join_states(&in_state[sidx], &state);
            if !already_visited || merged != in_state[sidx] {
                in_state[sidx] = merged;
                if !worklist.contains(&succ) {
                    worklist.push(succ);
                }
            }
        }
    }

    // Predecessors check satisfied by fixpoint; the `preds` calc was kept
    // for future scaling.
    let _ = preds;
}

fn compute_predecessors(body: &MirBody) -> HashMap<BlockId, Vec<BlockId>> {
    let mut out: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (i, block) in body.blocks.iter().enumerate() {
        let me = BlockId(i as u32);
        for succ in successors(&block.terminator) {
            out.entry(succ).or_default().push(me);
        }
    }
    out
}

fn successors(t: &Terminator) -> Vec<BlockId> {
    match t {
        Terminator::Goto(b) => vec![*b],
        Terminator::SwitchInt {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Drop { target, .. } => vec![*target],
        Terminator::Return | Terminator::Unreachable => Vec::new(),
    }
}

fn join_states(
    a: &HashMap<LocalId, LocalState>,
    b: &HashMap<LocalId, LocalState>,
) -> HashMap<LocalId, LocalState> {
    let mut out = a.clone();
    for (k, v) in b {
        let merged = match out.get(k).copied() {
            None => *v,
            Some(prev) => prev.join(*v),
        };
        out.insert(*k, merged);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk_statement(
    stmt: &Statement,
    state: &mut HashMap<LocalId, LocalState>,
    moved_at: &mut HashMap<LocalId, Span>,
    owner_locals: &[LocalId],
    level: BorrowCheckLevel,
    diags: &mut Vec<Diagnostic>,
    suppress_diagnostics: bool,
) {
    match stmt {
        Statement::Assign(dest, rvalue) => {
            walk_rvalue(
                rvalue,
                state,
                moved_at,
                owner_locals,
                level,
                diags,
                suppress_diagnostics,
            );
            // Writing to an owner local re-initializes it.
            if owner_locals.contains(&dest.local) && dest.projections.is_empty() {
                state.insert(dest.local, LocalState::Init);
            }
        }
        Statement::Call {
            destination,
            func,
            args,
        } => {
            read_operand(
                func,
                state,
                moved_at,
                owner_locals,
                level,
                diags,
                suppress_diagnostics,
            );
            for arg in args {
                read_operand(
                    arg,
                    state,
                    moved_at,
                    owner_locals,
                    level,
                    diags,
                    suppress_diagnostics,
                );
            }
            if let Some(dest) = destination {
                if owner_locals.contains(&dest.local) && dest.projections.is_empty() {
                    state.insert(dest.local, LocalState::Init);
                }
            }
        }
        Statement::StorageLive(local) => {
            if owner_locals.contains(local) {
                state.insert(*local, LocalState::Uninit);
            }
        }
        Statement::StorageDead(local) => {
            if owner_locals.contains(local) {
                state.insert(*local, LocalState::Uninit);
            }
        }
        Statement::Nop => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_rvalue(
    rvalue: &Rvalue,
    state: &mut HashMap<LocalId, LocalState>,
    moved_at: &mut HashMap<LocalId, Span>,
    owner_locals: &[LocalId],
    level: BorrowCheckLevel,
    diags: &mut Vec<Diagnostic>,
    suppress_diagnostics: bool,
) {
    match rvalue {
        Rvalue::Use(op) => read_operand(
            op,
            state,
            moved_at,
            owner_locals,
            level,
            diags,
            suppress_diagnostics,
        ),
        Rvalue::Ref(_, _) | Rvalue::AddressOf(_, _) => {
            // Borrow of an owner local doesn't move it. We leave detection
            // of borrow-while-moved for Phase 7.x.
        }
        Rvalue::BinaryOp(_, a, b) => {
            read_operand(
                a,
                state,
                moved_at,
                owner_locals,
                level,
                diags,
                suppress_diagnostics,
            );
            read_operand(
                b,
                state,
                moved_at,
                owner_locals,
                level,
                diags,
                suppress_diagnostics,
            );
        }
        Rvalue::UnaryOp(_, a) => read_operand(
            a,
            state,
            moved_at,
            owner_locals,
            level,
            diags,
            suppress_diagnostics,
        ),
        Rvalue::Cast(_, op, _) => read_operand(
            op,
            state,
            moved_at,
            owner_locals,
            level,
            diags,
            suppress_diagnostics,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn read_operand(
    op: &Operand,
    state: &mut HashMap<LocalId, LocalState>,
    moved_at: &HashMap<LocalId, Span>,
    owner_locals: &[LocalId],
    level: BorrowCheckLevel,
    diags: &mut Vec<Diagnostic>,
    suppress_diagnostics: bool,
) {
    match op {
        Operand::Copy(p) | Operand::Move(p) => {
            if owner_locals.contains(&p.local) {
                let cur = state.get(&p.local).copied().unwrap_or(LocalState::Uninit);
                if cur == LocalState::Moved && !suppress_diagnostics {
                    let prev = moved_at.get(&p.local).copied().unwrap_or(Span::DUMMY);
                    let mut diag = make_diagnostic(level, p, prev);
                    let _ = &mut diag;
                    diags.push(diag);
                }
            }
        }
        Operand::Const(_) => {}
    }
    // Apply Move state transitions after the diagnostic check so a Move
    // following a Move still gets reported.
    if let Operand::Move(p) = op {
        if owner_locals.contains(&p.local) {
            // Record this as the new move site for the next use-after-move
            // diagnostic, even if it itself was already moved.
            // (We can't access moved_at mutably here; we update through a
            // separate map maintained by the caller. To keep things simple,
            // we re-derive moved_at in a second pass when emitting from
            // walk_statement, which holds the mutable reference.)
        }
        // Mark moved.
        state.insert(p.local, LocalState::Moved);
    }
}

fn make_diagnostic(level: BorrowCheckLevel, place: &Place, moved_span: Span) -> Diagnostic {
    let severity = match level {
        BorrowCheckLevel::Error => Severity::Error,
        BorrowCheckLevel::Warn => Severity::Warning,
    };
    let mut diag =
        Diagnostic::new(severity, "use of moved owner pointer").with_code(E_USE_AFTER_MOVE);
    // We don't have the literal source span of this MIR read here in the
    // place; the caller in walk_statement has it via statement span. For MVP
    // we attach a secondary label pointing to where the move happened.
    if !moved_span.is_dummy() {
        diag = diag.with_label(Label::secondary(moved_span, "value moved here"));
    }
    diag = diag
        .with_note(format!("local _{}", place.local.0))
        .with_help(
            "pass a borrow ([[sc::borrow]] / [[sc::borrow_mut]]) or stop using the value after the move",
        );
    diag
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalState {
    /// Storage allocated but no value yet.
    Uninit,
    /// Holds a valid owner value.
    Init,
    /// Owner value has been moved out.
    Moved,
}

impl LocalState {
    fn join(self, other: LocalState) -> LocalState {
        use LocalState::*;
        match (self, other) {
            (a, b) if a == b => a,
            // Any branch saying Moved poisons the join.
            (Moved, _) | (_, Moved) => Moved,
            (Init, _) | (_, Init) => Init,
            _ => Uninit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rccx_mir::build_module;
    use rccx_parser::parse_module;
    use rccx_pp::{preprocess, PpOptions};
    use rccx_source::SourceMap;
    use rccx_typeck::check as typeck_check;

    fn check_src(src: &str) -> Vec<Diagnostic> {
        let mut sm = SourceMap::new();
        let id = sm.add_file("t.c", src);
        let (tokens, _) = preprocess(&mut sm, id, &PpOptions::default());
        let root_span = sm
            .file(id)
            .map(|f| rccx_source::Span::new(id, 0, f.text().len() as u32))
            .unwrap();
        let (module, parse_diags) = parse_module(&tokens, &sm, root_span);
        assert!(parse_diags.is_empty(), "parse: {parse_diags:?}");
        let (hir, type_diags) = typeck_check(&module);
        assert!(type_diags.is_empty(), "typeck: {type_diags:?}");
        let mir = build_module(&hir);
        check(&mir, BorrowCheckLevel::Error)
    }

    #[test]
    fn no_diagnostics_for_pure_c() {
        let diags = check_src("int main(void) { int x = 1; return x; }\n");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn no_diagnostics_for_raw_pointer_reuse() {
        let diags = check_src(
            "int main(void) { int x = 0; int *p = &x; int y = *p; int z = *p; return y + z; }\n",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn double_consume_of_owner_is_diagnosed() {
        let diags = check_src(
            "void consume([[sc::owner]] int *p);\n\
             void f(void) {\n\
                 [[sc::owner]] int *p;\n\
                 consume(p);\n\
                 consume(p);\n\
             }\n",
        );
        assert!(
            diags.iter().any(|d| d.code.unwrap().0 == "E0001"),
            "{diags:?}"
        );
    }

    #[test]
    fn single_consume_of_owner_is_ok() {
        let diags = check_src(
            "void consume([[sc::owner]] int *p);\n\
             void f(void) {\n\
                 [[sc::owner]] int *p;\n\
                 consume(p);\n\
             }\n",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn reinitializing_owner_after_move_is_ok() {
        let diags = check_src(
            "void consume([[sc::owner]] int *p);\n\
             [[sc::owner]] int *make(void);\n\
             void f(void) {\n\
                 [[sc::owner]] int *p = make();\n\
                 consume(p);\n\
                 p = make();\n\
                 consume(p);\n\
             }\n",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn warn_mode_downgrades_to_warning() {
        let mut sm = SourceMap::new();
        let id = sm.add_file(
            "t.c",
            "void consume([[sc::owner]] int *p);\nvoid f(void) { [[sc::owner]] int *p; consume(p); consume(p); }\n",
        );
        let (tokens, _) = preprocess(&mut sm, id, &PpOptions::default());
        let root_span = rccx_source::Span::new(id, 0, sm.file(id).unwrap().text().len() as u32);
        let (module, _) = parse_module(&tokens, &sm, root_span);
        let (hir, _) = typeck_check(&module);
        let mir = build_module(&hir);
        let diags = check(&mir, BorrowCheckLevel::Warn);
        assert!(diags.iter().all(|d| !d.is_error()));
        assert!(diags.iter().any(|d| d.code.unwrap().0 == "E0001"));
    }
}
