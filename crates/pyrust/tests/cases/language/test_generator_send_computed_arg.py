# Regression tests for issue #1438: generator send() value corrupted when a
# function call with a computed inline argument precedes a yield expression.
#
# Root cause: pass_copy_prop did not invalidate aliases written through the
# Yield{dst} register.  When the temp allocator reused the same register as
# a call's arg_reg for the subsequent yield dst, the old arg-result alias
# remained live across the yield and caused the sent value to be replaced by
# the previously computed argument value.

# --- 1. Basic trigger: id(x + 0) before second yield ---

def gen():
    x = yield 1
    id(x + 0)          # computed arg — retarget_last or Move into arg_reg
    y = yield 2
    yield y

g = gen()
next(g)
g.send(10)             # advances to 'yield 2'
result = g.send(20)
print(result)          # 20


# --- 2. f-string arg before second yield ---

def gen2():
    x = yield 1
    print(f"x={x}")    # f-string compiles to multiple instructions
    y = yield 2
    yield y

g2 = gen2()
next(g2)
g2.send(10)
print(g2.send(20))     # 20


# --- 3. list literal arg with a local variable ---

def gen3():
    x = yield 1
    id([x])             # BuildList with local: triggers retarget or Move
    y = yield 2
    yield y

g3 = gen3()
next(g3)
g3.send(10)
print(g3.send(20))     # 20


# --- 4. Non-trigger: literal arg (id("hello")) ---

def gen4():
    x = yield 1
    id("hello")         # literal: retarget_last fires into arg_reg, but no alias issue
    y = yield 2
    yield y

g4 = gen4()
next(g4)
g4.send(10)
print(g4.send(20))     # 20


# --- 5. Non-trigger: named variable (no inline computation) ---

def gen5():
    x = yield 1
    msg = f"x={x}"
    print(msg)          # named variable — no retarget into call frame
    y = yield 2
    yield y

g5 = gen5()
next(g5)
g5.send(10)
print(g5.send(20))     # 20


# --- 6. Unary op on local (single instruction, triggers retarget_last) ---

def gen6():
    x = yield 1
    id(-x)              # UnaryOp(temp, Neg, x_local): single insn, retarget fires
    y = yield 2
    yield y

g6 = gen6()
next(g6)
g6.send(10)
print(g6.send(20))     # 20


# --- 7. Multiple computed calls before the second yield ---

def gen7():
    x = yield 1
    id(x + 0)
    id(-x)
    y = yield 2
    yield y

g7 = gen7()
next(g7)
g7.send(10)
print(g7.send(20))     # 20
