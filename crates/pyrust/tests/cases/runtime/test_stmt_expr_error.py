# Expression statements that raise must propagate — not be silently dropped.
#
# The optimizer's pass_dead_store_elim previously removed BinOp, BinOpConst,
# UnaryOp, and LoadGlobal instructions whose temp-register destination was
# never read (because the result of the expression statement is discarded).
# This caused exceptions raised during evaluation to be swallowed.

import sys


def try_run(fn):
    """Run fn; print the exception class name if it raises, else print 'no error'."""
    try:
        fn()
        print("no error")
    except Exception as e:
        print(type(e).__name__)


# 1. Runtime BinOp: negative shift — ValueError
def test_runtime_lshift():
    a, b = 1, -1
    a << b   # result discarded; error must still propagate

try_run(test_runtime_lshift)

# 2. BinOpConst: division by zero — ZeroDivisionError
def test_const_divzero():
    1 / 0   # constant RHS; result discarded

try_run(test_const_divzero)

# 3. BinOp with constant lhs: 1 << -1 as statement
def test_const_lshift_neg():
    1 << -1   # both operands constant; result discarded

try_run(test_const_lshift_neg)

# 4. LoadGlobal of undefined name — NameError
def test_undefined_global():
    _surely_undefined_name_550   # noqa: F821

try_run(test_undefined_global)

# 5. UnaryOp that raises via __neg__ dunder
class _BadNeg:
    def __neg__(self):
        raise TypeError("no neg")

def test_unary_dunder():
    x = _BadNeg()
    -x   # result discarded; TypeError must propagate

try_run(test_unary_dunder)

# 6. Valid expression statement (function call, result discarded) must still work.
_call_count = 0
def _side_effect():
    global _call_count
    _call_count += 1
    return 42

def test_valid_call_discarded():
    _side_effect()   # result discarded; function must still be called
    print(_call_count)

test_valid_call_discarded()
