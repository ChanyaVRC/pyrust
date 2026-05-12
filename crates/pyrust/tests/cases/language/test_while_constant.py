# Constant-condition while loops: True/False/1/0/None

# while True — runs until explicit break
n = 0
while True:
    n += 1
    if n == 5:
        break
print("while-true-count", n)

# while 1 — identical semantics to while True
m = 0
while 1:
    m += 1
    if m == 3:
        break
print("while-one-count", m)

# while True — continue skips to next iteration
even_sum = 0
i = 0
while True:
    if i >= 8:
        break
    i += 1
    if i % 2 != 0:
        continue
    even_sum += i
print("while-true-even-sum", even_sum)

# while False — body never executes
x = 0
while False:
    x = 99
print("while-false-skip", x)

# while 0 — body never executes
y = 0
while 0:
    y = 99
print("while-zero-skip", y)

# while False with else — else branch always runs
flag = "init"
while False:
    flag = "body"
else:
    flag = "else"
print("while-false-else", flag)

# while True with nested break — outer loop controlled by flag
found = -1
outer_done = False
items = [10, 20, 30, 40]
i = 0
while True:
    if i >= len(items):
        break
    if items[i] == 30:
        found = i
        break
    i += 1
print("while-true-search", found)

# while True with else — else is unreachable (break skips it, condition never goes false)
trace = []
n = 0
while True:
    trace.append(n)
    n += 1
    if n == 3:
        break
else:
    trace.append("dead-else")
print("while-true-else-skip", trace)

# while 1 with else — same: else dead even though break is taken
seen = []
k = 0
while 1:
    seen.append(k)
    k += 1
    if k == 2:
        break
else:
    seen.append("never")
print("while-one-else-skip", seen)

# while True else with no break in sight: else still must never run (sanity guard)
# Using a function so the infinite loop is contained by an explicit early-return break.
def _inf_with_else():
    out = []
    while True:
        out.append("body")
        break
    else:
        out.append("else-bad")
    return out
print("while-true-fn-else", _inf_with_else())
