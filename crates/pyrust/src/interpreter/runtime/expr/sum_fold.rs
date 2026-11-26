/// CPython 3.12 `builtin_sum` accumulator state machine.
///
/// One canonical implementation shared by the `sum` builtin and by
/// `Insn::SumStep`, the per-element step of a compiler-inlined
/// `sum(<genexpr>)` (#3384), so the two can never drift apart:
///
///   * `Int` — CPython's C-long fast path.  It only admits exact `int` / `bool`
///     items and leaves for `Generic` on overflow or any other item, exactly
///     like the C loop (there is deliberately no BigInt fast accumulator, so
///     bit results stay identical when a big int later meets a float).
///   * `Float` — the running sum plus the Neumaier (Kahan–Babuška)
///     compensation term CPython 3.12 carries for float items.
///   * `Generic` — the plain `PyNumber_Add` fold, which is never left again.
pub(crate) enum SumAccumulator {
    Int(i64),
    Float(f64, f64),
    Generic(Value),
}

/// `f_result + c`, CPython's way: the compensation is skipped when it is zero
/// (so a `-0.0` running sum keeps its sign) or non-finite (so an infinite or
/// overflowed sum does not collapse to NaN).
#[inline]
fn sum_apply_compensation(sum: f64, compensation: f64) -> f64 {
    if compensation != 0.0 && compensation.is_finite() {
        sum + compensation
    } else {
        sum
    }
}

impl SumAccumulator {
    /// Seed the accumulator from `start` (`None` = the default `0`).
    ///
    /// CPython only enters the int fast path for an *exact* int
    /// (`PyLong_CheckExact`): a `bool` start is a subclass, so it — like every
    /// big-int or non-numeric start — begins in generic mode, which also keeps
    /// `sum([], start)` returning `start` itself.
    pub(crate) fn from_start(start: Option<&Value>) -> Self {
        match start {
            None => Self::Int(0),
            Some(value) => match value.kind() {
                ValueKind::Int(n) => Self::Int(n),
                ValueKind::Float(f) => Self::Float(f, 0.0),
                _ => Self::Generic(value.clone()),
            },
        }
    }

    /// Fold one element into the accumulator.
    pub(crate) fn add(&mut self, interp: &mut Interpreter, item: Value) -> Result<()> {
        // Classify the element without holding a borrow of `item` (so it can
        // still be moved into the generic fallback).  `bool` items DO take part
        // in the int/float fast paths (CPython's `PyBool_Check` branch); only
        // big ints / non-numerics break out.
        enum Num {
            Int(i64),
            Float(f64),
            Other,
        }
        let num = match item.kind() {
            ValueKind::Int(n) => Num::Int(n),
            ValueKind::Bool(b) => Num::Int(i64::from(b)),
            ValueKind::Float(f) => Num::Float(f),
            _ => Num::Other,
        };

        match self {
            Self::Int(sum) => match num {
                Num::Int(n) => match sum.checked_add(n) {
                    Some(total) => *sum = total,
                    // Overflow: fall to the generic loop with `sum + item`,
                    // exactly as CPython does (no BigInt fast path).
                    None => {
                        let current = Value::int(*sum);
                        *self = Self::Generic(interp.eval_binary(current, BinaryOp::Add, item)?);
                    }
                },
                // CPython seeds the float path with `(double)i_result + item`
                // and a zero compensation: compensation only kicks in from
                // the *next* element.
                Num::Float(f) => *self = Self::Float(*sum as f64 + f, 0.0),
                // Big int (or any non-fast item): continue in generic mode.
                Num::Other => {
                    let current = Value::int(*sum);
                    *self = Self::Generic(interp.eval_binary(current, BinaryOp::Add, item)?);
                }
            },
            Self::Float(sum, compensation) => match num {
                // Floats go through the Neumaier step, bit-identical to
                // CPython 3.12's float fast path …
                Num::Float(x) => {
                    let total = *sum + x;
                    *compensation += if sum.abs() >= x.abs() {
                        (*sum - total) + x
                    } else {
                        (x - total) + *sum
                    };
                    *sum = total;
                }
                // … but ints are added plainly (no compensation), exactly as
                // CPython's float loop handles small `PyLong` items.
                Num::Int(n) => *sum += n as f64,
                // Big int / non-numeric: hand `f_result + c` to the generic
                // loop (CPython rebuilds a PyFloat and calls PyNumber_Add).
                Num::Other => {
                    let current = Value::float(sum_apply_compensation(*sum, *compensation));
                    *self = Self::Generic(interp.eval_binary(current, BinaryOp::Add, item)?);
                }
            },
            Self::Generic(sum) => {
                let current = std::mem::replace(sum, Value::none());
                *sum = interp.eval_binary(current, BinaryOp::Add, item)?;
            }
        }
        Ok(())
    }

    /// The value `sum` returns once the iterable is exhausted.
    pub(crate) fn finish(self) -> Value {
        match self {
            Self::Int(sum) => Value::int(sum),
            Self::Float(sum, compensation) => {
                Value::float(sum_apply_compensation(sum, compensation))
            }
            Self::Generic(sum) => sum,
        }
    }
}

/// Slot holding the value `sum` would return if iteration stopped now.
const SUM_CELL_RESULT: usize = 0;
/// Slot holding the mode: `False` = int fast path (or a start value that has
/// not been classified yet), a `float` = the raw running float sum, `None` =
/// generic mode.
const SUM_CELL_MODE: usize = 1;
/// Slot holding the Neumaier compensation term while in float mode.
const SUM_CELL_COMPENSATION: usize = 2;
const SUM_CELL_LEN: usize = 3;

impl Interpreter {
    /// One `Insn::SumStep`: fold `item` into the three-slot accumulator cell
    /// `[result, mode, compensation]` the compiler allocates for an inlined
    /// `sum(<genexpr>)` (see [`crate::bytecode::Insn::SumStep`]).
    ///
    /// The cell is private to the compiler-synthesised reduction frame, so no
    /// Python code can observe or alias it; it exists only because a register
    /// cannot carry the float path's `(sum, compensation)` pair.
    pub(crate) fn sum_cell_step(&mut self, cell: &Value, item: Value) -> Result<()> {
        enum Decoded {
            /// The int fast path absorbed the item in place.
            Done,
            Pending(SumAccumulator),
        }
        let item_int = match item.kind() {
            ValueKind::Int(n) => Some(n),
            ValueKind::Bool(b) => Some(i64::from(b)),
            _ => None,
        };
        let decoded = cell.list_with_mut(|slots| {
            if slots.len() != SUM_CELL_LEN {
                return None;
            }
            enum Mode {
                /// Int fast path, or a start value not classified yet.
                IntOrUnseeded,
                Float(f64),
                Generic,
            }
            let mode = match slots[SUM_CELL_MODE].kind() {
                ValueKind::Bool(false) => Mode::IntOrUnseeded,
                ValueKind::Float(sum) => Mode::Float(sum),
                _ => Mode::Generic,
            };
            Some(match mode {
                Mode::IntOrUnseeded => {
                    // Hot path: an int sum stays an int — one borrow, no decode.
                    let int_total = match (slots[SUM_CELL_RESULT].kind(), item_int) {
                        (ValueKind::Int(sum), Some(n)) => sum.checked_add(n),
                        _ => None,
                    };
                    match int_total {
                        Some(total) => {
                            slots[SUM_CELL_RESULT] = Value::int(total);
                            Decoded::Done
                        }
                        None => Decoded::Pending(SumAccumulator::from_start(Some(
                            &slots[SUM_CELL_RESULT],
                        ))),
                    }
                }
                Mode::Float(sum) => {
                    let compensation = match slots[SUM_CELL_COMPENSATION].kind() {
                        ValueKind::Float(c) => c,
                        _ => 0.0,
                    };
                    Decoded::Pending(SumAccumulator::Float(sum, compensation))
                }
                Mode::Generic => Decoded::Pending(SumAccumulator::Generic(std::mem::replace(
                    &mut slots[SUM_CELL_RESULT],
                    Value::none(),
                ))),
            })
        });
        let mut accumulator = match decoded.flatten() {
            Some(Decoded::Pending(accumulator)) => accumulator,
            Some(Decoded::Done) => return Ok(()),
            None => {
                return Err(PyError::runtime(
                    "internal: SumStep operand is not a sum accumulator cell".to_string(),
                ));
            }
        };

        // The cell is not borrowed across `add`: a generic step can run
        // arbitrary `__add__` / `__radd__` code.
        accumulator.add(self, item)?;

        cell.list_with_mut(|slots| match accumulator {
            SumAccumulator::Int(sum) => {
                slots[SUM_CELL_RESULT] = Value::int(sum);
                slots[SUM_CELL_MODE] = Value::bool_(false);
            }
            SumAccumulator::Float(sum, compensation) => {
                slots[SUM_CELL_RESULT] = Value::float(sum_apply_compensation(sum, compensation));
                slots[SUM_CELL_MODE] = Value::float(sum);
                slots[SUM_CELL_COMPENSATION] = Value::float(compensation);
            }
            SumAccumulator::Generic(sum) => {
                slots[SUM_CELL_RESULT] = sum;
                slots[SUM_CELL_MODE] = Value::none();
            }
        });
        Ok(())
    }
}
