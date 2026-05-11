//! HIR → MIR lowering.

use rccx_hir::{self as hir, HirType};
use rccx_source::Span;

use crate::*;

/// Lower an entire `HirModule` to a `MirModule`. Only function definitions
/// (those with bodies) produce MIR; prototypes and globals are skipped.
pub fn build_module(module: &hir::HirModule) -> MirModule {
    let mut out = MirModule::default();
    for item in &module.items {
        if let hir::HirItem::FnDef(f) = item {
            if f.body.is_some() {
                let body = Builder::new(module, f).build();
                out.functions.push(body);
            }
        }
    }
    out
}

struct Builder<'a> {
    module: &'a hir::HirModule,
    /// The HIR function we're lowering.
    func: &'a hir::HirFnDef,
    /// Output locals (`locals[0]` is the return slot).
    locals: Vec<LocalDecl>,
    /// Output basic blocks.
    blocks: Vec<BasicBlock>,
    /// Which block subsequent statements should land in.
    current: BlockId,
    /// HIR symbol → MIR local.
    sym_to_local: SymbolLocals,
    /// Stack of `(continue_target, break_target)` pairs.
    loop_stack: Vec<(BlockId, BlockId)>,
}

impl<'a> Builder<'a> {
    fn new(module: &'a hir::HirModule, func: &'a hir::HirFnDef) -> Self {
        let mut me = Self {
            module,
            func,
            locals: Vec::new(),
            blocks: Vec::new(),
            current: BlockId::START,
            sym_to_local: SymbolLocals::new(),
            loop_stack: Vec::new(),
        };
        // Return slot.
        me.locals.push(LocalDecl {
            ty: func.return_type.clone(),
            name: Some("<return>".to_string()),
            kind: LocalKind::Return,
            span: func.span,
        });
        // Parameters.
        for p in &func.params {
            let name = module.symbols.get(p.sym).name.clone();
            let local = LocalId(me.locals.len() as u32);
            me.locals.push(LocalDecl {
                ty: p.ty.clone(),
                name: Some(name).filter(|s| !s.is_empty()),
                kind: LocalKind::Arg,
                span: p.span,
            });
            me.sym_to_local.insert(p.sym, local);
        }
        me
    }

    fn arg_count(&self) -> u32 {
        self.func.params.len() as u32
    }

    fn build(mut self) -> MirBody {
        // Entry block starts open (Unreachable as placeholder); we patch as
        // we lower and close it at the end if the user didn't `return`.
        self.new_block_with_terminator(Terminator::Unreachable);
        self.current = BlockId::START;

        if let Some(body) = &self.func.body {
            self.lower_block(body);
        }

        // If we fell off the end of the function body, close the trailing
        // block with `Return`. The caller can later inject a default
        // assignment to `_0` for non-void functions in Phase 6 / 8.
        if self.is_current_live() {
            self.set_terminator(self.current, Terminator::Return);
        }

        MirBody {
            sym: self.func.sym,
            name: self.module.symbols.get(self.func.sym).name.clone(),
            arg_count: self.arg_count(),
            locals: self.locals,
            blocks: self.blocks,
            span: self.func.span,
        }
    }

    // === Block management ===

    fn new_block_with_terminator(&mut self, t: Terminator) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock::new(t));
        id
    }

    /// Allocate a new block but place a temporary `Unreachable` terminator on
    /// it. Callers must patch it later.
    fn new_block(&mut self) -> BlockId {
        self.new_block_with_terminator(Terminator::Unreachable)
    }

    fn set_terminator(&mut self, block: BlockId, t: Terminator) {
        self.blocks[block.0 as usize].terminator = t;
    }

    fn push_stmt(&mut self, s: Statement) {
        self.blocks[self.current.0 as usize].statements.push(s);
    }

    /// Returns whether the current block has not yet been terminated by an
    /// earlier control-flow construct (e.g. a return inside an if branch).
    fn is_current_live(&self) -> bool {
        !matches!(
            self.blocks[self.current.0 as usize].terminator,
            Terminator::Return | Terminator::Goto(_) | Terminator::SwitchInt { .. }
        )
    }

    fn switch_to(&mut self, block: BlockId) {
        self.current = block;
    }

    // === Locals ===

    fn fresh_temp(&mut self, ty: HirType, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(LocalDecl {
            ty,
            name: None,
            kind: LocalKind::Temp,
            span,
        });
        id
    }

    fn declare_var(&mut self, sym: SymbolId, ty: HirType, name: String, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(LocalDecl {
            ty,
            name: Some(name),
            kind: LocalKind::Var,
            span,
        });
        self.sym_to_local.insert(sym, id);
        id
    }

    // === Statements ===

    fn lower_block(&mut self, block: &hir::HirBlock) {
        for s in &block.stmts {
            if !self.is_current_live() {
                break;
            }
            self.lower_stmt(s);
        }
    }

    fn lower_stmt(&mut self, s: &hir::HirStmt) {
        match &s.kind {
            hir::HirStmtKind::Compound(b) => self.lower_block(b),
            hir::HirStmtKind::Expr(e) => {
                // Side-effecting expression statement.
                let _ = self.lower_expr_to_operand(e);
            }
            hir::HirStmtKind::Decl(decls) => {
                for d in decls {
                    self.lower_decl(d);
                }
            }
            hir::HirStmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if(cond, then_branch, else_branch.as_deref()),
            hir::HirStmtKind::While { cond, body } => self.lower_while(cond, body),
            hir::HirStmtKind::DoWhile { body, cond } => self.lower_do_while(body, cond),
            hir::HirStmtKind::For {
                init,
                cond,
                step,
                body,
            } => self.lower_for(init.as_ref(), cond.as_ref(), step.as_ref(), body),
            hir::HirStmtKind::Return(opt) => self.lower_return(opt.as_ref(), s.span),
            hir::HirStmtKind::Break => {
                if let Some((_, brk)) = self.loop_stack.last().copied() {
                    self.set_terminator(self.current, Terminator::Goto(brk));
                }
            }
            hir::HirStmtKind::Continue => {
                if let Some((cont, _)) = self.loop_stack.last().copied() {
                    self.set_terminator(self.current, Terminator::Goto(cont));
                }
            }
            hir::HirStmtKind::Empty => {}
        }
    }

    fn lower_decl(&mut self, d: &hir::HirDecl) {
        let name = self.module.symbols.get(d.sym).name.clone();
        let local = self.declare_var(d.sym, d.ty.clone(), name, d.span);
        self.push_stmt(Statement::StorageLive(local));
        if let Some(init) = &d.init {
            let op = self.lower_expr_to_operand(init);
            self.push_stmt(Statement::Assign(Place::from_local(local), Rvalue::Use(op)));
        }
    }

    fn lower_return(&mut self, value: Option<&hir::HirExpr>, _span: Span) {
        if let Some(e) = value {
            let op = self.lower_expr_to_operand(e);
            let ret = Place::from_local(LocalId(0));
            self.push_stmt(Statement::Assign(ret, Rvalue::Use(op)));
        }
        self.set_terminator(self.current, Terminator::Return);
    }

    fn lower_if(
        &mut self,
        cond: &hir::HirExpr,
        then_branch: &hir::HirStmt,
        else_branch: Option<&hir::HirStmt>,
    ) {
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join = self.new_block();

        let cond_op = self.lower_expr_to_operand(cond);
        self.set_terminator(
            self.current,
            Terminator::SwitchInt {
                cond: cond_op,
                then_block,
                else_block,
            },
        );

        // Then arm.
        self.switch_to(then_block);
        self.lower_stmt(then_branch);
        if self.is_current_live() {
            self.set_terminator(self.current, Terminator::Goto(join));
        }

        // Else arm.
        self.switch_to(else_block);
        if let Some(e) = else_branch {
            self.lower_stmt(e);
        }
        if self.is_current_live() {
            self.set_terminator(self.current, Terminator::Goto(join));
        }

        self.switch_to(join);
    }

    fn lower_while(&mut self, cond: &hir::HirExpr, body: &hir::HirStmt) {
        let header = self.new_block();
        let body_block = self.new_block();
        let exit = self.new_block();

        self.set_terminator(self.current, Terminator::Goto(header));
        self.switch_to(header);
        let cond_op = self.lower_expr_to_operand(cond);
        self.set_terminator(
            self.current,
            Terminator::SwitchInt {
                cond: cond_op,
                then_block: body_block,
                else_block: exit,
            },
        );

        self.loop_stack.push((header, exit));
        self.switch_to(body_block);
        self.lower_stmt(body);
        if self.is_current_live() {
            self.set_terminator(self.current, Terminator::Goto(header));
        }
        self.loop_stack.pop();

        self.switch_to(exit);
    }

    fn lower_do_while(&mut self, body: &hir::HirStmt, cond: &hir::HirExpr) {
        let body_block = self.new_block();
        let cond_block = self.new_block();
        let exit = self.new_block();

        self.set_terminator(self.current, Terminator::Goto(body_block));
        self.loop_stack.push((cond_block, exit));
        self.switch_to(body_block);
        self.lower_stmt(body);
        if self.is_current_live() {
            self.set_terminator(self.current, Terminator::Goto(cond_block));
        }
        self.loop_stack.pop();

        self.switch_to(cond_block);
        let cond_op = self.lower_expr_to_operand(cond);
        self.set_terminator(
            self.current,
            Terminator::SwitchInt {
                cond: cond_op,
                then_block: body_block,
                else_block: exit,
            },
        );

        self.switch_to(exit);
    }

    fn lower_for(
        &mut self,
        init: Option<&hir::HirForInit>,
        cond: Option<&hir::HirExpr>,
        step: Option<&hir::HirExpr>,
        body: &hir::HirStmt,
    ) {
        if let Some(init) = init {
            match init {
                hir::HirForInit::Decl(ds) => {
                    for d in ds {
                        self.lower_decl(d);
                    }
                }
                hir::HirForInit::Expr(e) => {
                    let _ = self.lower_expr_to_operand(e);
                }
            }
        }

        let header = self.new_block();
        let body_block = self.new_block();
        let step_block = self.new_block();
        let exit = self.new_block();

        self.set_terminator(self.current, Terminator::Goto(header));
        self.switch_to(header);
        if let Some(c) = cond {
            let op = self.lower_expr_to_operand(c);
            self.set_terminator(
                self.current,
                Terminator::SwitchInt {
                    cond: op,
                    then_block: body_block,
                    else_block: exit,
                },
            );
        } else {
            self.set_terminator(self.current, Terminator::Goto(body_block));
        }

        self.loop_stack.push((step_block, exit));
        self.switch_to(body_block);
        self.lower_stmt(body);
        if self.is_current_live() {
            self.set_terminator(self.current, Terminator::Goto(step_block));
        }
        self.loop_stack.pop();

        self.switch_to(step_block);
        if let Some(s) = step {
            let _ = self.lower_expr_to_operand(s);
        }
        self.set_terminator(self.current, Terminator::Goto(header));

        self.switch_to(exit);
    }

    // === Expressions ===

    /// Lower an expression to an [`Operand`]. If the expression is an lvalue
    /// the result references the place; otherwise a fresh temporary is
    /// allocated and the result of the computation written to it.
    fn lower_expr_to_operand(&mut self, e: &hir::HirExpr) -> Operand {
        if let Some(place) = self.try_lower_as_place(e) {
            return read_place(&place, &e.ty);
        }
        self.lower_expr_to_value(e)
    }

    /// Lower an expression that produces a value (not an lvalue) into a
    /// fresh temporary, returning a `Copy` operand for it.
    fn lower_expr_to_value(&mut self, e: &hir::HirExpr) -> Operand {
        match &e.kind {
            hir::HirExprKind::IntLit(v) => Operand::Const(Constant::Int(*v, e.ty.clone())),
            hir::HirExprKind::FloatLit(v) => Operand::Const(Constant::Float(*v, e.ty.clone())),
            hir::HirExprKind::CharLit(v) => Operand::Const(Constant::Char(*v)),
            hir::HirExprKind::StringLit(s) => Operand::Const(Constant::Str(s.clone())),
            hir::HirExprKind::Ref(sym) => self.lower_symbol_use(*sym, &e.ty, e.span),
            hir::HirExprKind::Error => Operand::Const(Constant::Error),
            hir::HirExprKind::Call { callee, args } => self.lower_call(callee, args, &e.ty, e.span),
            hir::HirExprKind::Binary { op, lhs, rhs } => {
                self.lower_binary(*op, lhs, rhs, &e.ty, e.span)
            }
            hir::HirExprKind::Unary { op, operand } => {
                self.lower_unary(*op, operand, &e.ty, e.span)
            }
            hir::HirExprKind::Postfix { op, operand } => {
                self.lower_postfix(*op, operand, &e.ty, e.span)
            }
            hir::HirExprKind::Assign { op, lhs, rhs } => {
                self.lower_assign(*op, lhs, rhs, &e.ty, e.span)
            }
            hir::HirExprKind::Ternary {
                cond,
                then_branch,
                else_branch,
            } => self.lower_ternary(cond, then_branch, else_branch, &e.ty, e.span),
            hir::HirExprKind::Cast { target, expr } => {
                let inner = self.lower_expr_to_operand(expr);
                self.emit_rvalue(
                    Rvalue::Cast(CastKind::Explicit, inner, target.clone()),
                    &e.ty,
                    e.span,
                )
            }
            hir::HirExprKind::ImplicitCast { target, expr } => {
                let inner = self.lower_expr_to_operand(expr);
                let kind = if expr.ty.is_array() {
                    CastKind::ArrayToPointer
                } else {
                    CastKind::Implicit
                };
                self.emit_rvalue(Rvalue::Cast(kind, inner, target.clone()), &e.ty, e.span)
            }
            hir::HirExprKind::Index { .. } => {
                // Index is normally an lvalue; if it falls through to here,
                // treat the read as a Copy of the projected place.
                let place = self
                    .try_lower_as_place(e)
                    .unwrap_or_else(|| Place::from_local(self.fresh_temp(e.ty.clone(), e.span)));
                Operand::Copy(place)
            }
            hir::HirExprKind::SizeofExpr(_) => {
                // We don't compute real type sizes yet; use a placeholder
                // constant so codegen sees something well-typed.
                Operand::Const(Constant::Int(8, e.ty.clone()))
            }
            hir::HirExprKind::Comma(exprs) => {
                let mut last = Operand::Const(Constant::Error);
                for inner in exprs {
                    last = self.lower_expr_to_operand(inner);
                }
                last
            }
        }
    }

    fn lower_symbol_use(&mut self, sym: SymbolId, ty: &HirType, span: Span) -> Operand {
        // Function symbols become a constant function reference; everything
        // else is a place read.
        let symbol = self.module.symbols.get(sym);
        if matches!(symbol.kind, hir::SymbolKind::Function) {
            Operand::Const(Constant::FnRef(sym, ty.clone()))
        } else if let Some(local) = self.sym_to_local.get(&sym).copied() {
            read_place(&Place::from_local(local), ty)
        } else {
            // Global variable we haven't reified into a local. For MVP we
            // skip emitting a real read and emit an error placeholder so the
            // dump shows something sensible.
            let _ = span;
            Operand::Const(Constant::Error)
        }
    }

    fn try_lower_as_place(&mut self, e: &hir::HirExpr) -> Option<Place> {
        match &e.kind {
            hir::HirExprKind::Ref(sym) => {
                self.sym_to_local.get(sym).copied().map(Place::from_local)
            }
            hir::HirExprKind::Unary {
                op: hir::HirUnaryOp::Deref,
                operand,
            } => {
                let inner = self.lower_expr_to_operand(operand);
                let local = self.materialize_operand(inner, &operand.ty, operand.span);
                Some(Place::from_local(local).with_deref())
            }
            hir::HirExprKind::Index { base, index } => {
                let base_op = self.lower_expr_to_operand(base);
                let base_local = self.materialize_operand(base_op, &base.ty, base.span);
                let idx_op = self.lower_expr_to_operand(index);
                let idx_local = self.materialize_operand(idx_op, &index.ty, index.span);
                Some(
                    Place::from_local(base_local)
                        .with_deref()
                        .with_index(idx_local),
                )
            }
            hir::HirExprKind::ImplicitCast { expr, .. } | hir::HirExprKind::Cast { expr, .. } => {
                // Casts don't preserve lvalue-ness — leave to rvalue path.
                let _ = expr;
                None
            }
            _ => None,
        }
    }

    fn materialize_operand(&mut self, op: Operand, ty: &HirType, span: Span) -> LocalId {
        match &op {
            Operand::Copy(p) | Operand::Move(p) if p.projections.is_empty() => p.local,
            _ => {
                let l = self.fresh_temp(ty.clone(), span);
                self.push_stmt(Statement::Assign(Place::from_local(l), Rvalue::Use(op)));
                l
            }
        }
    }

    fn emit_rvalue(&mut self, rvalue: Rvalue, ty: &HirType, span: Span) -> Operand {
        let dest = self.fresh_temp(ty.clone(), span);
        self.push_stmt(Statement::Assign(Place::from_local(dest), rvalue));
        read_place(&Place::from_local(dest), ty)
    }

    fn lower_call(
        &mut self,
        callee: &hir::HirExpr,
        args: &[hir::HirExpr],
        ret_ty: &HirType,
        span: Span,
    ) -> Operand {
        let callee_op = self.lower_expr_to_operand(callee);
        let arg_ops: Vec<_> = args.iter().map(|a| self.lower_expr_to_operand(a)).collect();
        if ret_ty.is_void() {
            self.push_stmt(Statement::Call {
                destination: None,
                func: callee_op,
                args: arg_ops,
            });
            Operand::Const(Constant::Error)
        } else {
            let dest = self.fresh_temp(ret_ty.clone(), span);
            self.push_stmt(Statement::Call {
                destination: Some(Place::from_local(dest)),
                func: callee_op,
                args: arg_ops,
            });
            Operand::Copy(Place::from_local(dest))
        }
    }

    fn lower_binary(
        &mut self,
        op: hir::HirBinaryOp,
        lhs: &hir::HirExpr,
        rhs: &hir::HirExpr,
        ty: &HirType,
        span: Span,
    ) -> Operand {
        // Short-circuit logical operators expand to control flow.
        if matches!(
            op,
            hir::HirBinaryOp::LogicalAnd | hir::HirBinaryOp::LogicalOr
        ) {
            return self.lower_short_circuit(op, lhs, rhs, ty, span);
        }
        let l = self.lower_expr_to_operand(lhs);
        let r = self.lower_expr_to_operand(rhs);
        let bin = match op {
            hir::HirBinaryOp::Add => BinOp::Add,
            hir::HirBinaryOp::Sub => BinOp::Sub,
            hir::HirBinaryOp::Mul => BinOp::Mul,
            hir::HirBinaryOp::Div => BinOp::Div,
            hir::HirBinaryOp::Mod => BinOp::Mod,
            hir::HirBinaryOp::Shl => BinOp::Shl,
            hir::HirBinaryOp::Shr => BinOp::Shr,
            hir::HirBinaryOp::BitAnd => BinOp::BitAnd,
            hir::HirBinaryOp::BitXor => BinOp::BitXor,
            hir::HirBinaryOp::BitOr => BinOp::BitOr,
            hir::HirBinaryOp::Eq => BinOp::Eq,
            hir::HirBinaryOp::NotEq => BinOp::NotEq,
            hir::HirBinaryOp::Lt => BinOp::Lt,
            hir::HirBinaryOp::Gt => BinOp::Gt,
            hir::HirBinaryOp::LtEq => BinOp::LtEq,
            hir::HirBinaryOp::GtEq => BinOp::GtEq,
            hir::HirBinaryOp::LogicalAnd | hir::HirBinaryOp::LogicalOr => unreachable!(),
        };
        self.emit_rvalue(Rvalue::BinaryOp(bin, l, r), ty, span)
    }

    fn lower_short_circuit(
        &mut self,
        op: hir::HirBinaryOp,
        lhs: &hir::HirExpr,
        rhs: &hir::HirExpr,
        ty: &HirType,
        span: Span,
    ) -> Operand {
        // Materialize the result in a temporary so we can produce a single
        // operand at the end.
        let result = self.fresh_temp(ty.clone(), span);
        let rhs_block = self.new_block();
        let join_block = self.new_block();

        let lhs_op = self.lower_expr_to_operand(lhs);
        let (then_b, else_b, short_value) = match op {
            hir::HirBinaryOp::LogicalAnd => (rhs_block, join_block, 0_i128),
            hir::HirBinaryOp::LogicalOr => (join_block, rhs_block, 1_i128),
            _ => unreachable!(),
        };
        self.set_terminator(
            self.current,
            Terminator::SwitchInt {
                cond: lhs_op,
                then_block: then_b,
                else_block: else_b,
            },
        );

        // Short-circuit path: write the short-circuit value into `result`.
        let short_block = if matches!(op, hir::HirBinaryOp::LogicalAnd) {
            else_b
        } else {
            then_b
        };
        self.switch_to(short_block);
        self.push_stmt(Statement::Assign(
            Place::from_local(result),
            Rvalue::Use(Operand::Const(Constant::Int(short_value, ty.clone()))),
        ));
        self.set_terminator(self.current, Terminator::Goto(join_block));

        // RHS path.
        self.switch_to(rhs_block);
        let rhs_op = self.lower_expr_to_operand(rhs);
        self.push_stmt(Statement::Assign(
            Place::from_local(result),
            Rvalue::Use(rhs_op),
        ));
        self.set_terminator(self.current, Terminator::Goto(join_block));

        self.switch_to(join_block);
        Operand::Copy(Place::from_local(result))
    }

    fn lower_unary(
        &mut self,
        op: hir::HirUnaryOp,
        operand: &hir::HirExpr,
        ty: &HirType,
        span: Span,
    ) -> Operand {
        match op {
            hir::HirUnaryOp::Plus => self.lower_expr_to_operand(operand),
            hir::HirUnaryOp::Neg => {
                let inner = self.lower_expr_to_operand(operand);
                self.emit_rvalue(Rvalue::UnaryOp(UnOp::Neg, inner), ty, span)
            }
            hir::HirUnaryOp::LogicalNot => {
                let inner = self.lower_expr_to_operand(operand);
                self.emit_rvalue(Rvalue::UnaryOp(UnOp::Not, inner), ty, span)
            }
            hir::HirUnaryOp::BitNot => {
                let inner = self.lower_expr_to_operand(operand);
                self.emit_rvalue(Rvalue::UnaryOp(UnOp::BitNot, inner), ty, span)
            }
            hir::HirUnaryOp::AddrOf => {
                let place = self.try_lower_as_place(operand).unwrap_or_else(|| {
                    Place::from_local(self.fresh_temp(operand.ty.clone(), operand.span))
                });
                self.emit_rvalue(Rvalue::AddressOf(BorrowKind::Shared, place), ty, span)
            }
            hir::HirUnaryOp::Deref => {
                // Deref normally produces an lvalue; fall back to a Copy of
                // the derefed place if we end up here.
                let place = self
                    .try_lower_as_place(&hir::HirExpr {
                        kind: hir::HirExprKind::Unary {
                            op: hir::HirUnaryOp::Deref,
                            operand: Box::new(operand.clone()),
                        },
                        ty: ty.clone(),
                        span,
                    })
                    .unwrap_or_else(|| Place::from_local(self.fresh_temp(ty.clone(), span)));
                Operand::Copy(place)
            }
            hir::HirUnaryOp::PreInc | hir::HirUnaryOp::PreDec => {
                let bin = if matches!(op, hir::HirUnaryOp::PreInc) {
                    BinOp::Add
                } else {
                    BinOp::Sub
                };
                let place = self
                    .try_lower_as_place(operand)
                    .expect("typeck guarantees an lvalue here");
                let cur = read_place(&place, ty);
                let one = Operand::Const(Constant::Int(1, ty.clone()));
                let new = self.emit_rvalue(Rvalue::BinaryOp(bin, cur, one), ty, span);
                self.push_stmt(Statement::Assign(place.clone(), Rvalue::Use(new.clone())));
                new
            }
        }
    }

    fn lower_postfix(
        &mut self,
        op: hir::HirPostfixOp,
        operand: &hir::HirExpr,
        ty: &HirType,
        span: Span,
    ) -> Operand {
        let place = self
            .try_lower_as_place(operand)
            .expect("typeck guarantees an lvalue here");
        let old = read_place(&place, ty);
        // Save old value.
        let saved = self.fresh_temp(ty.clone(), span);
        self.push_stmt(Statement::Assign(
            Place::from_local(saved),
            Rvalue::Use(old.clone()),
        ));
        // Compute new.
        let bin = match op {
            hir::HirPostfixOp::Inc => BinOp::Add,
            hir::HirPostfixOp::Dec => BinOp::Sub,
        };
        let cur = read_place(&place, ty);
        let one = Operand::Const(Constant::Int(1, ty.clone()));
        let new = self.emit_rvalue(Rvalue::BinaryOp(bin, cur, one), ty, span);
        self.push_stmt(Statement::Assign(place, Rvalue::Use(new)));
        Operand::Copy(Place::from_local(saved))
    }

    fn lower_assign(
        &mut self,
        op: hir::HirAssignOp,
        lhs: &hir::HirExpr,
        rhs: &hir::HirExpr,
        ty: &HirType,
        span: Span,
    ) -> Operand {
        let place = self
            .try_lower_as_place(lhs)
            .expect("typeck guarantees an lvalue here");
        let rhs_op = self.lower_expr_to_operand(rhs);
        let new_value = match op {
            hir::HirAssignOp::Assign => rhs_op,
            other => {
                let bin = compound_assign_binop(other);
                let lhs_read = read_place(&place, ty);
                self.emit_rvalue(Rvalue::BinaryOp(bin, lhs_read, rhs_op), ty, span)
            }
        };
        self.push_stmt(Statement::Assign(
            place.clone(),
            Rvalue::Use(new_value.clone()),
        ));
        Operand::Copy(place)
    }

    fn lower_ternary(
        &mut self,
        cond: &hir::HirExpr,
        then_e: &hir::HirExpr,
        else_e: &hir::HirExpr,
        ty: &HirType,
        span: Span,
    ) -> Operand {
        let result = self.fresh_temp(ty.clone(), span);
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join = self.new_block();

        let cond_op = self.lower_expr_to_operand(cond);
        self.set_terminator(
            self.current,
            Terminator::SwitchInt {
                cond: cond_op,
                then_block,
                else_block,
            },
        );

        self.switch_to(then_block);
        let t_op = self.lower_expr_to_operand(then_e);
        self.push_stmt(Statement::Assign(
            Place::from_local(result),
            Rvalue::Use(t_op),
        ));
        self.set_terminator(self.current, Terminator::Goto(join));

        self.switch_to(else_block);
        let e_op = self.lower_expr_to_operand(else_e);
        self.push_stmt(Statement::Assign(
            Place::from_local(result),
            Rvalue::Use(e_op),
        ));
        self.set_terminator(self.current, Terminator::Goto(join));

        self.switch_to(join);
        Operand::Copy(Place::from_local(result))
    }
}

fn compound_assign_binop(op: hir::HirAssignOp) -> BinOp {
    match op {
        hir::HirAssignOp::AddAssign => BinOp::Add,
        hir::HirAssignOp::SubAssign => BinOp::Sub,
        hir::HirAssignOp::MulAssign => BinOp::Mul,
        hir::HirAssignOp::DivAssign => BinOp::Div,
        hir::HirAssignOp::ModAssign => BinOp::Mod,
        hir::HirAssignOp::ShlAssign => BinOp::Shl,
        hir::HirAssignOp::ShrAssign => BinOp::Shr,
        hir::HirAssignOp::AndAssign => BinOp::BitAnd,
        hir::HirAssignOp::XorAssign => BinOp::BitXor,
        hir::HirAssignOp::OrAssign => BinOp::BitOr,
        hir::HirAssignOp::Assign => unreachable!(),
    }
}

fn read_place(place: &Place, ty: &HirType) -> Operand {
    // Phase 7: owner pointers move on read; plain raw pointers and borrows
    // are read by copy. The borrow checker reads the resulting `Move` to
    // detect use-after-move.
    if matches!(ty.ownership(), rccx_hir::Ownership::Owner) {
        Operand::Move(place.clone())
    } else {
        Operand::Copy(place.clone())
    }
}
