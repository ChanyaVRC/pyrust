# Compile-time SyntaxError for try/except* misuse (issues #2147, #2174):
#   * mixing `except` and `except*` on one `try`
#   * return/break/continue inside an `except*` block


def check(src, mode="exec"):
    try:
        compile(src, "<test>", mode)
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)


# Mixing except and except* (#2147).
check("try:\n pass\nexcept* ValueError:\n pass\nexcept TypeError:\n pass")
check("try:\n pass\nexcept ValueError:\n pass\nexcept* TypeError:\n pass")

# return / break / continue directly inside except* (#2174).
check("def f():\n try: pass\n except* ValueError:\n  return 1")
check("for i in range(3):\n try: pass\n except* ValueError:\n  break")
check("for i in range(3):\n try: pass\n except* ValueError:\n  continue")
check(
    "def f():\n for i in range(3):\n  try: pass\n  except* ValueError:\n   break"
)

# Valid neighbors: a pure except* handler, and break/continue/return bound to a
# loop / function nested *inside* the except* block.
try:
    raise ExceptionGroup("g", [ValueError(1)])
except* ValueError as eg:
    print(eg.exceptions[0].args)

try:
    pass
except* ValueError:
    while True:
        break
    for _ in range(2):
        continue
print("except* control-flow ok")


def has_nested_def():
    result = 0
    try:
        pass
    except* ValueError:

        def g():
            return 1

        result = g()
    return result


print(has_nested_def())

