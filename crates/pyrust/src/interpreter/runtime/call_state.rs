// Active Python frame depth is thread-wide: native calls, call trampolines,
// generator drives, and nested Interpreter entries all contribute to the same
// logical recursion chain. The configurable recursion limit belongs to the
// active Interpreter.
thread_local! {
    static CALL_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// Address identity of the Interpreter whose execution entry currently
    /// supplies pyrust-core's dynamically-scoped integer conversion limit.
    ///
    /// The pointer is comparison-only and is never dereferenced. An executing
    /// `&mut Interpreter` cannot move for the duration of its entry method.
    static ACTIVE_INT_MAX_STR_DIGITS_OWNER: Cell<usize> = const { Cell::new(0) };
}

pub(crate) fn get_recursion_limit(interpreter: &Interpreter) -> usize {
    interpreter.recursion_limit
}

pub(crate) fn set_recursion_limit(interpreter: &mut Interpreter, limit: usize) {
    interpreter.recursion_limit = limit;
}

pub(crate) fn get_int_max_str_digits(interpreter: &Interpreter) -> usize {
    interpreter.int_max_str_digits
}

/// Update the interpreter-owned digit limit and, when this interpreter is the
/// active dynamic owner, immediately publish it to pyrust-core.
pub(crate) fn set_int_max_str_digits(interpreter: &mut Interpreter, limit: usize) {
    interpreter.int_max_str_digits = limit;
    let owner = interpreter as *const Interpreter as usize;
    let is_active = ACTIVE_INT_MAX_STR_DIGITS_OWNER.with(|active| active.get() == owner);
    if is_active {
        pyrust_core::set_int_max_str_digits(limit);
    }
}

/// Installs one Interpreter's integer conversion policy for an execution
/// entry, restoring both the previous owner and core TLS value on drop.
///
/// Nested `exec`/`eval` on the same Interpreter is a no-op so a
/// `sys.set_int_max_str_digits` performed inside it remains visible to the
/// surrounding execution. A genuinely different nested Interpreter receives
/// its own scope and restores the outer interpreter afterward.
pub(super) struct IntMaxStrDigitsExecutionGuard {
    previous_owner: usize,
    core_guard: Option<pyrust_core::IntMaxStrDigitsGuard>,
}

impl IntMaxStrDigitsExecutionGuard {
    pub(super) fn enter(interpreter: &Interpreter) -> Self {
        let owner = interpreter as *const Interpreter as usize;
        let previous_owner = ACTIVE_INT_MAX_STR_DIGITS_OWNER.with(Cell::get);
        if previous_owner == owner {
            return Self {
                previous_owner,
                core_guard: None,
            };
        }

        let core_guard = pyrust_core::scoped_int_max_str_digits(interpreter.int_max_str_digits);
        ACTIVE_INT_MAX_STR_DIGITS_OWNER.with(|active| active.set(owner));
        Self {
            previous_owner,
            core_guard: Some(core_guard),
        }
    }
}

impl Drop for IntMaxStrDigitsExecutionGuard {
    fn drop(&mut self) {
        if let Some(core_guard) = self.core_guard.take() {
            drop(core_guard);
            ACTIVE_INT_MAX_STR_DIGITS_OWNER.with(|active| active.set(self.previous_owner));
        }
    }
}

pub(super) fn max_call_depth(interpreter: &Interpreter) -> usize {
    get_recursion_limit(interpreter)
}

pub(super) fn call_depth() -> usize {
    CALL_DEPTH.with(Cell::get)
}

pub(crate) fn get_call_depth() -> usize {
    call_depth()
}

/// Counts one active Python frame and restores the thread-local depth on every
/// exit path, including panics.
pub(super) struct CallDepthGuard;

impl CallDepthGuard {
    pub(super) fn enter() -> Self {
        CALL_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

impl Drop for CallDepthGuard {
    fn drop(&mut self) {
        CALL_DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

#[cfg(test)]
mod int_max_str_digits_execution_tests {
    use super::{IntMaxStrDigitsExecutionGuard, Interpreter, set_int_max_str_digits};

    #[test]
    fn interpreter_scopes_reuse_same_owner_and_restore_nested_other_owner() {
        let _host_limit = pyrust_core::scoped_int_max_str_digits(0);
        let mut first = Interpreter::default();
        let mut second = Interpreter::default();
        set_int_max_str_digits(&mut first, 640);
        set_int_max_str_digits(&mut second, 1000);
        assert_eq!(pyrust_core::get_int_max_str_digits(), 0);

        {
            let _first_scope = IntMaxStrDigitsExecutionGuard::enter(&first);
            assert_eq!(pyrust_core::get_int_max_str_digits(), 640);

            {
                let _same_owner = IntMaxStrDigitsExecutionGuard::enter(&first);
                set_int_max_str_digits(&mut first, 0);
                assert_eq!(pyrust_core::get_int_max_str_digits(), 0);
            }
            // A same-owner nested entry must not restore the old 640 setting.
            assert_eq!(pyrust_core::get_int_max_str_digits(), 0);
            set_int_max_str_digits(&mut first, 640);

            {
                let _second_scope = IntMaxStrDigitsExecutionGuard::enter(&second);
                assert_eq!(pyrust_core::get_int_max_str_digits(), 1000);
            }
            assert_eq!(pyrust_core::get_int_max_str_digits(), 640);
        }
        assert_eq!(pyrust_core::get_int_max_str_digits(), 0);
    }
}
