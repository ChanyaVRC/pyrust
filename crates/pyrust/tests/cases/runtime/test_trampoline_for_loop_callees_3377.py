# Call trampoline into callees that contain `for` loops (#3377).
#
# A trampolined callee runs inside its caller's dispatch loop, so the caller's
# loop-iterator slots have to be parked for the duration of the call and handed
# back on return and on error unwind.  Each section pins one way that hand-off
# can go wrong: a caller loop resuming on the callee's iterator, a stale
# `__next__` cache, an unwind that resumes a generator consumer on the wrong
# slots, or an iterator that outlives the frame that owned it.
import traceback


def total(xs):
    s = 0
    for x in xs:
        s += x
    return s


def pairs(xs, ys):
    out = []
    for x in xs:
        for y in ys:
            out.append((x, y))
    return out


print("== caller loop survives callee loops ==")


def caller_loop(rows):
    out = []
    for row in rows:
        out.append(total(row))
        for extra in row:
            out.append(total([extra, extra]))
    return out


print(caller_loop([[1, 2, 3], [], [10], (4, 5)]))
print(pairs("ab", range(3)))


def nested_caller(n):
    acc = []
    for i in range(n):
        for j in range(i):
            acc.append(len(pairs(range(i), range(j))) + total(range(j)))
    return acc


print(nested_caller(5))

print("== module-level loop calling loop callees ==")
seen = []
for chunk in ([1], [2, 3], [4, 5, 6]):
    seen.append(total(chunk))
    for item in chunk:
        seen.append(total(range(item)))
print(seen)

print("== recursion through loops ==")


def flatten(tree):
    out = []
    for node in tree:
        if isinstance(node, list):
            for leaf in flatten(node):
                out.append(leaf)
        else:
            out.append(node)
    return out


print(flatten([1, [2, [3, [4, [5]], 6], 7], [], [[8]], 9]))


def permutations(items):
    if len(items) <= 1:
        return [list(items)]
    out = []
    for i in range(len(items)):
        for rest in permutations(items[:i] + items[i + 1:]):
            out.append([items[i]] + rest)
    return out


print(permutations("abc"))


def depth_sum(n):
    s = 0
    for i in range(2):
        if n > 0:
            s += depth_sum(n - 1) + i
    return s


print(depth_sum(8))

print("== break / continue / else / early return ==")


def first_even(xs):
    for x in xs:
        if x % 2:
            continue
        return x
    return None


def find(xs, needle):
    for i, x in enumerate(xs):
        if x == needle:
            break
    else:
        return -1
    return i


def first_pair_summing(xs, target):
    for a in xs:
        for b in xs:
            if a + b == target:
                return (a, b)
    return None


def loop_control(rows):
    out = []
    for row in rows:
        if not row:
            continue
        out.append((first_even(row), find(row, 3), first_pair_summing(row, 7)))
        if len(out) == 3:
            break
    else:
        out.append("no break")
    return out


print(loop_control([[1, 3, 5], [], [2, 3], [3, 4], [9, 9]]))
print(loop_control([[1], [5, 2]]))

print("== user iterators keep their own __next__ ==")


class Up:
    def __init__(self, n):
        self.i = 0
        self.n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self.i >= self.n:
            raise StopIteration
        self.i += 1
        return self.i


class Down:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self.n <= 0:
            raise StopIteration
        self.n -= 1
        return -self.n


def drain_down(n):
    out = []
    for v in Down(n):
        out.append(v)
    return out


def drain_up_calling_down(n):
    out = []
    for v in Up(n):
        out.append((v, drain_down(v)))
    return out


print(drain_up_calling_down(4))

print("== methods with loops ==")


class Matrix:
    def __init__(self, rows):
        self.rows = rows

    def row_sum(self, i):
        s = 0
        for v in self.rows[i]:
            s += v
        return s

    def sums(self):
        out = []
        for i in range(len(self.rows)):
            out.append(self.row_sum(i))
        return out


m = Matrix([[1, 2], [3, 4, 5], []])
print(m.sums())
for index in range(3):
    print(m.row_sum(index), end=" ")
print()

print("== generators driven inside trampolined loops ==")


def countdown(n):
    for i in range(n, 0, -1):
        yield i


def squares(xs):
    for x in xs:
        yield total([x] * x)


def drain(gen):
    out = []
    for v in gen:
        out.append(v)
    return out


def drain_many(sizes):
    out = []
    for n in sizes:
        out.append(drain(countdown(n)))
        for sq in squares(range(n)):
            out.append(total(drain(countdown(sq))))
    return out


print(drain_many([0, 1, 3, 4]))


def peek_first(gen):
    for v in gen:
        return v
    return "empty"


def peeks():
    out = []
    for n in range(4):
        out.append(peek_first(countdown(n)))
    return out


print(peeks())

print("== errors raised inside trampolined loops ==")


def explode_at(xs, bad):
    for x in xs:
        if x == bad:
            raise ValueError("bad %r" % (x,))
    return len(xs)


def outer_explode(rows, bad):
    done = []
    for row in rows:
        done.append(explode_at(row, bad))
    return done


for bad in (99, 5, 1):
    try:
        print(outer_explode([[1, 2], [3, 4, 5], [6]], bad))
    except ValueError as exc:
        print("caught", exc)
    print(outer_explode([[7], [8, 9]], 99))


def exploding_gen(rows, bad):
    for row in rows:
        yield explode_at(row, bad)


def resume_after_unwind(rows):
    log = []
    for bad in (2, 99, 4):
        try:
            for n in exploding_gen(rows, bad):
                log.append(n)
        except ValueError as exc:
            log.append(str(exc))
        log.append(total([bad, 1]))
    return log


print(resume_after_unwind([[1], [2, 3], [4]]))


def raises_stop(xs):
    for x in xs:
        raise StopIteration(x)


def gen_calling_raises_stop():
    for i in range(3):
        yield i
        raises_stop([i])


def pep479():
    out = []
    for attempt in range(2):
        try:
            for v in gen_calling_raises_stop():
                out.append(v)
        except RuntimeError as exc:
            out.append((type(exc).__name__, str(exc), type(exc.__cause__).__name__))
    return out


print(pep479())

print("== tracebacks through trampolined loop frames ==")


def tb_leaf(xs):
    for x in xs:
        if x == 2:
            return 1 // (x - 2)
    return 0


def tb_mid(rows):
    acc = 0
    for row in rows:
        acc += tb_leaf(row)
    return acc


def tb_top():
    for rows in ([[1]], [[1], [1, 2]]):
        tb_mid(rows)


try:
    tb_top()
except ZeroDivisionError:
    lines = traceback.format_exc().splitlines()
    for line in lines:
        if set(line.strip()) <= set("^~"):
            # PEP 657 carets are pinned by the tracebacks/ fixtures.
            continue
        if line.lstrip().startswith("File "):
            print(line.split(", ", 1)[1])
        else:
            print(line)

print("== a generator abandoned by an early return stays resumable ==")


def take_first(gen):
    for v in gen:
        return v
    return "done"


def resume_shared():
    shared = countdown(4)
    out = []
    for _ in range(6):
        out.append(take_first(shared))
    return out


print(resume_shared())
