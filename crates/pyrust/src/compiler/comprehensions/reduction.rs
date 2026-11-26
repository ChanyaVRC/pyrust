// Guarded inlining of `sum` / `any` / `all` / `min` / `max` over a generator
// expression argument (#3384).
//
// `sum(x for x in xs)` normally builds a generator and hands it to the native
// builtin, which then resumes the generator frame once per element through
// `next()`.  When the callee value is the canonical builtin, the reduction can
// run *inside* the generator frame instead and hand back one result:
//
//     f  = <load `sum`>                  # NameError / shadowing as usual
//     it = <outermost iterable>          # evaluated in the enclosing scope
//     if f is <canonical builtin sum>:   # value guard — never trusts the name
//         g = <reduction genexpr>(it)    # same scope rules as the real genexpr
//         for r in g: ...                # one resume runs the whole reduction
//         result = <decode r>
//     else:
//         result = f(<the real genexpr>(it))
//
// The reduction frame is still a real `<genexpr>` *generator* frame, so every
// observable property of evaluating the element expressions is unchanged:
// scope isolation, class-scope visibility, PEP 572 targets, traceback frames,
// and the PEP 479 `StopIteration` → `RuntimeError` conversion.
//
// What CPython runs inside the builtin — the `+`, the truth test, the `<` /
// `>` comparison — must instead look as if it ran in the *enclosing* frame.
// Each such step is therefore wrapped in a handler that strips the reduction
// frame's own traceback entry and yields the exception back, and the enclosing
// frame re-raises it at the call site.

/// A reduction builtin the compiler can fold over a generator expression.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GenexprReduction {
    Sum,
    Any,
    All,
    Min,
    Max,
}

impl GenexprReduction {
    fn from_callee_name(name: &str) -> Option<Self> {
        Some(match name {
            "sum" => Self::Sum,
            "any" => Self::Any,
            "all" => Self::All,
            "min" => Self::Min,
            "max" => Self::Max,
            _ => return None,
        })
    }

    fn builtin_name(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Any => "any",
            Self::All => "all",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

/// The `<builtin>(<genexpr>[, extra])` call shape eligible for the reduction
/// lowering.  Everything else — `key=`, `start=` by keyword, splats, an
/// asynchronous generator expression — keeps the ordinary call.
struct GenexprReductionCall<'a> {
    kind: GenexprReduction,
    elt: &'a Expr,
    elt_lineno: u32,
    first_lineno: u32,
    clauses: &'a [CompClause],
    /// `sum`'s positional `start`, or `min` / `max`'s `default=`.
    extra: Option<&'a Expr>,
}

fn genexpr_reduction_call<'a>(
    func: &Expr,
    args: &'a [CallArg],
) -> Option<GenexprReductionCall<'a>> {
    let Expr::Var(name, _) = func else {
        return None;
    };
    let kind = GenexprReduction::from_callee_name(name.as_str())?;
    let (first, rest) = args.split_first()?;
    if first.name.is_some() || first.splat || first.double_splat {
        return None;
    }
    let Expr::GenExp {
        elt,
        elt_lineno,
        first_lineno,
        clauses,
    } = &first.value
    else {
        return None;
    };
    let extra = match (kind, rest) {
        (_, []) => None,
        (GenexprReduction::Sum, [start])
            if start.name.is_none() && !start.splat && !start.double_splat =>
        {
            Some(&start.value)
        }
        (GenexprReduction::Min | GenexprReduction::Max, [default])
            if default.name.is_some_and(|name| name.as_str() == "default")
                && !default.splat
                && !default.double_splat =>
        {
            Some(&default.value)
        }
        _ => return None,
    };
    Some(GenexprReductionCall {
        kind,
        elt,
        elt_lineno: *elt_lineno,
        first_lineno: *first_lineno,
        clauses,
        extra,
    })
}

// Names private to the synthesised reduction frame.  The leading `.` keeps
// them out of reach of user code, like the `.0` iterable parameter.
const REDUCTION_ACC: &str = ".acc";
const REDUCTION_ELT: &str = ".v";
const REDUCTION_HAS: &str = ".has";
const REDUCTION_EXC: &str = ".e";
/// Second implicit parameter of a `sum(<genexpr>, start)` reduction frame.
const REDUCTION_START_PARAM: &str = ".1";
/// Pseudo-method on the accumulator cell that lowers to `Insn::SumStep`.
const REDUCTION_SUM_STEP: &str = "sum_step";

fn reduction_var(name: &str) -> Expr {
    Expr::Var(Symbol::intern(name), None)
}

fn reduction_assign(name: &str, value: Expr) -> Stmt {
    Stmt::Assign(AssignTarget::Name(Symbol::intern(name)), value)
}

fn reduction_if(cond: Expr, body: Vec<Stmt>, else_branch: Option<Vec<Stmt>>) -> Stmt {
    Stmt::If {
        branches: vec![(cond, body)],
        else_branch,
        branch_linenos: vec![],
        else_linenos: vec![],
    }
}

fn reduction_compare(left: &str, op: crate::ast::CmpOp, right: Expr) -> Expr {
    Expr::Compare {
        left: Box::new(reduction_var(left)),
        ops: vec![(op, right)],
    }
}

/// `yield (flag, payload)` — the reduction frame's one message to the
/// enclosing frame: `(True, result)`, `(None, None)` for an empty `min` /
/// `max`, or `(False, exception)` for a failed reduction step.
fn reduction_yield(flag: Expr, payload: Expr, lineno: u32) -> Stmt {
    Stmt::Expr(Expr::Yield {
        value: Some(Box::new(Expr::Tuple(vec![flag, payload]))),
        span: None,
        lineno,
    })
}

/// Report `result` and finish the reduction frame.
fn reduction_finish(result: Expr, lineno: u32) -> Vec<Stmt> {
    vec![
        reduction_yield(Expr::Bool(true), result, lineno),
        Stmt::Return(None),
    ]
}

/// Run `step` — the part of the reduction CPython executes inside the builtin
/// — so that an exception it raises surfaces in the *enclosing* frame.
///
/// The handler catches everything (a bare `except` that still binds the
/// exception), drops the reduction frame's own entry from the traceback, and
/// yields the exception back for the enclosing frame to re-raise.  Frames
/// below the reduction frame (a user `__add__`, `__bool__`, `__lt__`, …) stay
/// in the traceback, exactly as when the builtin itself made the call.
fn reduction_protected(step: Vec<Stmt>, lineno: u32) -> Stmt {
    let traceback = Symbol::intern("__traceback__");
    let exc = || reduction_var(REDUCTION_EXC);
    let handler = vec![
        Stmt::AttrAssign {
            target: exc(),
            name: traceback,
            expr: Expr::Attr {
                target: Box::new(Expr::Attr {
                    target: Box::new(exc()),
                    name: traceback,
                    span: None,
                }),
                name: Symbol::intern("tb_next"),
                span: None,
            },
            span: None,
        },
        reduction_yield(Expr::Bool(false), exc(), lineno),
        Stmt::Return(None),
    ];
    Stmt::Try {
        body: step,
        handlers: vec![crate::ast::ExceptHandler {
            kind: None,
            name: Some(Symbol::intern(REDUCTION_EXC)),
            body: handler,
            is_star: false,
            body_linenos: vec![],
        }],
        else_branch: None,
        finally_branch: None,
        body_linenos: vec![],
        else_linenos: vec![],
        finally_linenos: vec![],
    }
}

impl GenexprReduction {
    /// Statements run before the loop nest.
    fn prologue(self, has_start: bool) -> Vec<Stmt> {
        match self {
            // `[result, mode, compensation]` — see `Insn::SumStep`.
            Self::Sum => vec![reduction_assign(
                REDUCTION_ACC,
                Expr::List(vec![
                    if has_start {
                        reduction_var(REDUCTION_START_PARAM)
                    } else {
                        Expr::Int(0)
                    },
                    Expr::Bool(false),
                    Expr::Float(0.0),
                ]),
            )],
            Self::Any | Self::All => vec![],
            Self::Min | Self::Max => vec![
                reduction_assign(REDUCTION_HAS, Expr::Bool(false)),
                reduction_assign(REDUCTION_ACC, Expr::None),
            ],
        }
    }

    /// Statements run for each produced element.  The element expression is
    /// evaluated first, *outside* any protected step, so its own exceptions
    /// keep the `<genexpr>` frame like in the real generator.
    fn per_element(self, elt: &Expr, lineno: u32) -> Vec<Stmt> {
        use crate::ast::CmpOp;
        let mut body = vec![reduction_assign(REDUCTION_ELT, elt.clone())];
        match self {
            Self::Sum => body.push(reduction_protected(
                vec![Stmt::Expr(Expr::Call {
                    span: None,
                    lineno: 0,
                    func: Box::new(Expr::Attr {
                        target: Box::new(reduction_var(REDUCTION_ACC)),
                        name: Symbol::intern(REDUCTION_SUM_STEP),
                        span: None,
                    }),
                    args: vec![CallArg {
                        name: None,
                        value: reduction_var(REDUCTION_ELT),
                        splat: false,
                        double_splat: false,
                    }],
                })],
                lineno,
            )),
            Self::Any | Self::All => {
                // `hit` is the truth value that ends the reduction early.
                let hit = self == Self::Any;
                // Exact bools — what comparisons and predicates produce —
                // need no truth-test protocol, so they skip the protected
                // step entirely.
                body.push(reduction_if(
                    reduction_compare(REDUCTION_ELT, CmpOp::Is, Expr::Bool(hit)),
                    reduction_finish(Expr::Bool(hit), lineno),
                    None,
                ));
                body.push(reduction_if(
                    reduction_compare(REDUCTION_ELT, CmpOp::IsNot, Expr::Bool(!hit)),
                    vec![
                        // The truth test (`__bool__` / `__len__`) is the
                        // builtin's own step; `not` materialises it as a bool.
                        reduction_protected(
                            vec![reduction_assign(
                                REDUCTION_ELT,
                                Expr::Unary {
                                    op: UnaryOp::Not,
                                    expr: Box::new(reduction_var(REDUCTION_ELT)),
                                    span: None,
                                },
                            )],
                            lineno,
                        ),
                        reduction_if(
                            reduction_compare(REDUCTION_ELT, CmpOp::Is, Expr::Bool(!hit)),
                            reduction_finish(Expr::Bool(hit), lineno),
                            None,
                        ),
                    ],
                    None,
                ));
            }
            Self::Min | Self::Max => {
                // CPython replaces the running best only on a strict
                // `item < best` (`min`) / `item > best` (`max`), which keeps
                // the first of several equal elements.
                let op = if self == Self::Min {
                    CmpOp::Lt
                } else {
                    CmpOp::Gt
                };
                body.push(reduction_if(
                    reduction_var(REDUCTION_HAS),
                    vec![reduction_protected(
                        vec![reduction_if(
                            reduction_compare(REDUCTION_ELT, op, reduction_var(REDUCTION_ACC)),
                            vec![reduction_assign(
                                REDUCTION_ACC,
                                reduction_var(REDUCTION_ELT),
                            )],
                            None,
                        )],
                        lineno,
                    )],
                    Some(vec![
                        reduction_assign(REDUCTION_ACC, reduction_var(REDUCTION_ELT)),
                        reduction_assign(REDUCTION_HAS, Expr::Bool(true)),
                    ]),
                ));
            }
        }
        body
    }

    /// Statements run once the loop nest is exhausted.
    fn epilogue(self, lineno: u32) -> Vec<Stmt> {
        match self {
            Self::Sum => vec![reduction_yield(
                Expr::Bool(true),
                Expr::Index {
                    target: Box::new(reduction_var(REDUCTION_ACC)),
                    index: Box::new(Expr::Int(0)),
                    span: None,
                },
                lineno,
            )],
            Self::Any => vec![reduction_yield(Expr::Bool(true), Expr::Bool(false), lineno)],
            Self::All => vec![reduction_yield(Expr::Bool(true), Expr::Bool(true), lineno)],
            Self::Min | Self::Max => vec![reduction_if(
                reduction_var(REDUCTION_HAS),
                vec![reduction_yield(
                    Expr::Bool(true),
                    reduction_var(REDUCTION_ACC),
                    lineno,
                )],
                Some(vec![reduction_yield(Expr::None, Expr::None, lineno)]),
            )],
        }
    }
}

impl Compiler {
    /// Lower `Insn::SumStep` from the synthesised `.acc.sum_step(.v)` call of
    /// a `sum` reduction frame.  `.acc` is compiler-internal and cannot appear
    /// in user source.
    fn try_emit_sum_reduction_step(&mut self, expr: &Expr) -> bool {
        self.try_emit_comp_acc_method(
            self.is_sum_reduction,
            REDUCTION_SUM_STEP,
            expr,
            Insn::SumStep,
        )
    }

    /// Compile `sum` / `any` / `all` / `min` / `max` applied directly to a
    /// generator expression as a guarded in-generator reduction (#3384).
    ///
    /// Returns `None` — leaving the ordinary call lowering to run — when the
    /// call is not an eligible shape or the callee name is statically known
    /// not to be the builtin.
    fn try_compile_genexpr_reduction(
        &mut self,
        func: &Expr,
        args: &[CallArg],
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Option<Reg> {
        let call = genexpr_reduction_call(func, args)?;
        let kind = call.kind;
        // A name this scope binds or captures is not the builtin.  This is
        // only a shortcut: the runtime guard below checks the loaded *value*,
        // so a rebound module global or patched `builtins` attribute is
        // caught regardless of what the name looked like here.
        let callee = Symbol::intern(kind.builtin_name());
        if self.local_index.contains_key(callee.as_str())
            || self.cell_slot_map.contains_key(&callee)
            || self.enclosing_cell_names.contains(&callee)
        {
            return None;
        }
        // An invalid generator expression is diagnosed by the ordinary path;
        // an asynchronous one is not iterable by these builtins at all.
        if self.failed
            || call.clauses.is_empty()
            || check_comprehension(&[call.elt], call.clauses, "generator expression").is_some()
            || call.clauses.iter().any(|clause| clause.is_async)
            || Self::comp_body_has_await(&[call.elt], call.clauses)
        {
            return None;
        }

        let nargs: u8 = if call.extra.is_some() { 2 } else { 1 };
        let func_reg = self.reserve_call_frame(1 + Reg::from(nargs))?;
        let frame_top = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = frame_top;

        // CPython evaluates the outermost iterable while building the
        // generator — before any later argument.  Snapshot it into a temp when
        // such an argument follows, since that expression may rebind the name.
        let iter_src = self.compile_expr(&call.clauses[0].iter);
        let iter_reg = if call.extra.is_some() && iter_src < self.base_temp {
            let snapshot = self.alloc_temp();
            self.emit(Insn::Move(snapshot, iter_src));
            snapshot
        } else {
            iter_src
        };
        if let Some(extra) = call.extra {
            self.compile_call_arg_into(extra, func_reg + 2);
        }
        if self.failed {
            return Some(func_reg);
        }
        let paths_base = self.next_temp;
        let paths_iter_depth = self.iter_depth;
        let saved_lineno = self.current_lineno;
        let call_lineno = if lineno != 0 { lineno } else { saved_lineno };

        // ── Guard: the callee value is the canonical builtin ────────────────
        let canonical = self.intern_const(Value::builtin_function(kind.builtin_name()));
        let guard_failed = self.emit(Insn::CmpJumpIfFalseConst(
            func_reg,
            BinaryOp::Is,
            canonical,
            0,
        ));

        // ── Fast path: run the reduction inside the generator frame ─────────
        let start_reg = if kind == GenexprReduction::Sum && call.extra.is_some() {
            // `sum((), start)` returns `start` after the builtin's own
            // str / bytes / bytearray rejection, raised here in the enclosing
            // frame before any element is produced — as in CPython.
            let check = self.reserve_call_frame(3)?;
            let empty = self.intern_const(Value::tuple(vec![]));
            self.emit(Insn::Move(check, func_reg));
            self.emit(Insn::LoadConst(check + 1, empty));
            self.emit(Insn::Move(check + 2, func_reg + 2));
            self.set_lineno(call_lineno);
            self.set_col_span_for_next(span);
            self.emit(Insn::Call(check, 2));
            self.set_lineno(saved_lineno);
            self.next_temp = check + 1;
            Some(check)
        } else {
            None
        };
        let gen_reg = self.compile_genexpr_reduction_frame(&call, iter_reg, start_reg);
        if self.failed {
            return Some(func_reg);
        }
        // Drive the generator to completion from this frame: the first resume
        // runs the whole reduction and yields its one message, the second lets
        // the frame finish so its iterators are released right here.
        let slot = self.alloc_iter();
        self.set_lineno(call_lineno);
        self.set_col_span_for_next(span);
        self.emit(Insn::GetIter(slot, gen_reg));
        let message = gen_reg;
        self.set_col_span_for_next(span);
        let exhausted_early = self.emit(Insn::ForIter(message, slot, 0));
        let scratch = self.alloc_temp();
        self.set_col_span_for_next(span);
        let exhausted = self.emit(Insn::ForIter(scratch, slot, 0));
        self.free_temp(scratch);
        self.patch_jump(exhausted_early);
        self.patch_jump(exhausted);
        self.free_iter();

        // Decode `(flag, payload)`.
        let flag = self.reserve_call_frame(2)?;
        let payload = flag + 1;
        self.emit(Insn::Unpack(flag, message, 2));
        let not_ok = self.emit(Insn::JumpIfFalse(flag, 0));
        self.emit(Insn::Move(func_reg, payload));
        let mut done = vec![self.emit(Insn::Jump(0))];
        self.patch_jump(not_ok);
        if matches!(kind, GenexprReduction::Min | GenexprReduction::Max) {
            // `(None, None)`: no element was produced.
            let none = self.intern_const(Value::none());
            let step_failed = self.emit(Insn::CmpJumpIfFalseConst(flag, BinaryOp::Is, none, 0));
            if call.extra.is_some() {
                self.emit(Insn::Move(func_reg, func_reg + 2));
            } else {
                // Let the builtin itself raise its empty-iterable ValueError.
                let empty = self.intern_const(Value::tuple(vec![]));
                self.emit(Insn::LoadConst(func_reg + 1, empty));
                self.set_col_span_for_next(span);
                self.emit(Insn::Call(func_reg, 1));
            }
            done.push(self.emit(Insn::Jump(0)));
            self.patch_jump(step_failed);
        }
        // `(False, exception)`: a reduction step failed; raise it from here.
        self.set_col_span_for_next(span);
        self.emit(Insn::RaiseValue(payload));
        self.set_lineno(saved_lineno);

        // ── Slow path: the original generator + call ────────────────────────
        self.patch_jump(guard_failed);
        self.next_temp = paths_base;
        self.iter_depth = paths_iter_depth;
        let generator = self.compile_gen_exp_over(
            iter_reg,
            call.elt,
            call.elt_lineno,
            call.first_lineno,
            call.clauses,
            false,
        );
        if self.failed {
            return Some(func_reg);
        }
        self.emit(Insn::Move(func_reg + 1, generator));
        self.set_lineno(call_lineno);
        self.set_col_span_for_next(span);
        if call.extra.is_some() && kind != GenexprReduction::Sum {
            let kwnames_idx = self.intern_const(Value::tuple(vec![Value::string("default")]));
            self.emit(Insn::CallKw {
                func: func_reg,
                total: 2,
                nkw: 1,
                kwnames_idx,
            });
        } else {
            self.emit(Insn::Call(func_reg, nargs));
        }
        self.set_lineno(saved_lineno);

        for jump in done {
            self.patch_jump(jump);
        }
        self.next_temp = func_reg + 1;
        Some(func_reg)
    }

    /// Build and instantiate the reduction generator for `call`, returning the
    /// register holding the (not yet started) generator object.
    fn compile_genexpr_reduction_frame(
        &mut self,
        call: &GenexprReductionCall<'_>,
        iter_reg: Reg,
        start_reg: Option<Reg>,
    ) -> Reg {
        let first_lineno = if call.first_lineno != 0 {
            call.first_lineno
        } else {
            self.current_lineno
        };
        let elt_lineno = if call.elt_lineno != 0 {
            call.elt_lineno
        } else {
            first_lineno
        };
        let kind = call.kind;
        let (loop_nest, loop_linenos) = Self::build_comp_loop_body_with_linenos(
            call.clauses,
            kind.per_element(call.elt, elt_lineno),
            elt_lineno,
        );
        let mut body = kind.prologue(start_reg.is_some());
        let mut body_linenos = vec![0; body.len()];
        body.extend(loop_nest);
        body_linenos.extend(loop_linenos);
        body.extend(kind.epilogue(elt_lineno));

        let qualname = self.genexpr_qualname();
        self.compile_comprehension_fn(CompFnConfig {
            iter_reg,
            extra_arg: start_reg.map(|reg| (REDUCTION_START_PARAM, reg)),
            fn_body: &body,
            body_linenos: &body_linenos,
            display_name: "<genexpr>",
            qualname: Some(&qualname),
            is_async: false,
            is_inlined_comp: false,
            is_set_comp: false,
            is_list_comp: false,
            is_sum_reduction: kind == GenexprReduction::Sum,
            list_comp_presize: false,
            first_lineno,
            call_span: None,
            comp_outer_for_span: None,
            await_result: false,
        })
    }
}
