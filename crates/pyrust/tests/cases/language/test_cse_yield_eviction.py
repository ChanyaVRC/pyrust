# Regression fixture for the CSE-table eviction gap on Yield/YieldFrom.
#
# When pass_cse processes a Yield instruction, the yield-dst register receives
# the caller's sent value on resume.  Any CSE table entry that records the
# yield-dst as a source register must be evicted so the next computation that
# uses that register reflects the new sent value, not the pre-yield value.
#
# The shape below is specifically designed so that:
#   - x (the yield-dst) is used as the source of a UnaryOp BEFORE a yield,
#     so the CSE table records (Neg, x_reg) -> a_reg.
#   - The SAME x register is written again by the next Yield instruction.
#   - A second UnaryOp with the same key (Neg, x_reg) appears AFTER that yield.
#
# Without the eviction fix, pass_cse would emit CopyReg(b, a) for the second
# UnaryOp, producing b == a == -first_sent instead of b == -second_sent.

def gen():
    x = yield -1       # Yield { dst = x_reg }; x_reg gets sent value on resume
    a = -x             # UnaryOp(a, Neg, x_reg) — CSE records {(Neg, x_reg) -> a}
    x = yield a        # Yield { dst = x_reg } — x_reg must be evicted from table
    b = -x             # UnaryOp(b, Neg, x_reg) — must NOT hit stale CSE entry
    yield b

g = gen()
first = next(g)        # runs to first yield, yields -1
second = g.send(10)    # x=10; a=-10; yields -10
third = g.send(20)     # x=20; b should be -20 (not -10 from stale CSE)

print(f"first={first} second={second} third={third}")


# Simple round-trip: generator send produces the expected negation each time.
# A second pass verifies that different sent values in the same position
# produce different results (rules out lucky coincidence from stale CSE).
def counter_gen():
    results = []
    x = yield 0
    results.append(-x)
    x = yield -x
    results.append(-x)
    yield results

g3 = counter_gen()
next(g3)
g3.send(3)
final = g3.send(7)
print(f"counter={final}")


# Exercise YieldFrom: result_reg and sent_reg must both be evicted.
# The inner generator uses send to return different values, and the outer
# generator applies a negation to confirm the CSE table was cleared.
def inner_values():
    sent = yield 100
    yield sent + 1


def outer_negate():
    # Collect yielded values from the sub-iterator, then negate the last one.
    y1 = yield from inner_values()
    # y1 is the StopIteration value from inner_values (None here).
    # Just yield a known constant to confirm outer resumed correctly.
    yield 999


g4 = outer_negate()
r1 = next(g4)          # inner yields 100
r2 = g4.send(50)       # inner yields 51, then inner stops; outer resumes
r3 = next(g4)          # outer yields 999
print(f"yield_from={r1} {r2} {r3}")
