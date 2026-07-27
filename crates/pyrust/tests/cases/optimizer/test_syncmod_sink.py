# Parity fixture for guarded int-loop versioning and SyncModuleGlobal deferral.
# After optimization, straight-line module-level int loops may execute an
# out-of-line copy whose namespace synchronization is deferred to loop exit.

i = 0
n = 100
s = 0
while i < n:
    s += i
    i += 1
print(s)  # 4950

acc = 0
for _ in range(50):
    acc += 1
print(acc)  # 50


# A branch can make the first *executed* assignment order differ from lexical
# order. The live globals dict exposes first-insertion order, so sync-bearing
# loops with interior control flow must remain on the original per-assignment
# path: iteration zero inserts order_y, iteration one inserts order_x.
namespace = globals()
order_i = 0
while order_i < 2:
    if order_i == 1:
        order_x = 1
    if order_i == 0:
        order_y = 1
    order_i += 1
namespace_keys = list(namespace)
print("conditional insertion order:", namespace_keys.index("order_y") < namespace_keys.index("order_x"))


# The entry guard's non-int edge runs the untouched original loop. In
# particular, an exception raised by the original header must retain the
# surrounding try handler after zero-cost exception-table construction.
class HeaderFailure(Exception):
    pass


class RaisingCounter:
    def __lt__(self, other):
        raise HeaderFailure("header")


try:
    guarded_counter = RaisingCounter()
    while guarded_counter < 2:
        guarded_counter += 1
except HeaderFailure as exc:
    print("guard fallback caught:", str(exc))


# A value may start as an i64-sized exact int and overflow into BigInt inside
# the specialized copy. Primitive promotion must stay exact and must not force
# protocol dispatch or truncate the result.
overflow_i = 0
overflow_value = 2**63 - 1
while overflow_i < 2:
    overflow_value += 1
    overflow_i += 1
print("versioned overflow:", overflow_value == 2**63 + 1)


# bool is not an exact int for the entry guard. The original Python operation
# owns bool-to-int promotion on the first augmented assignment.
bool_counter = False
while bool_counter < 2:
    bool_counter += 1
print("bool fallback:", bool_counter, type(bool_counter).__name__)


# Compiler if-break fusion must evaluate truthiness exactly once per test and
# preserve while-else break routing.
truth_calls = []


class BreakGate:
    def __bool__(self):
        truth_calls.append(len(truth_calls))
        return len(truth_calls) == 3


gate = BreakGate()
break_iterations = 0
while True:
    if gate:
        break
    break_iterations += 1
else:
    print("unreachable else")
print("if-break truthiness:", break_iterations, len(truth_calls))


# Function-local loops have no module syncs, so a primitive if-break may leave
# the fast copy through an external exit stub. Exercise break, ordinary exit,
# and the copied header's zero-trip edge.
def break_loop(limit):
    break_i = 0
    break_total = 0
    while break_i < limit:
        break_i += 1
        if break_i == 4:
            break
        break_total += break_i
    return break_i, break_total


print("versioned break:", break_loop(10), break_loop(3), break_loop(0))


# A continue edge re-enters the original header and is deliberately rejected
# by the versioning pass; compiler direct-conditional lowering must nevertheless
# preserve the re-check and final result.
def continue_loop(limit):
    continue_i = 0
    continue_total = 0
    while continue_i < limit:
        continue_i += 1
        if continue_i == 2:
            continue
        continue_total += continue_i
    return continue_i, continue_total


print("continue fallback:", continue_loop(5), continue_loop(0))
