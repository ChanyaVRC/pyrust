# Parity fixture for issue #1747: 'await' outside an async context must raise
# SyntaxError with the correct CPython 3.12 message.
#
# CPython 3.12 distinguishes two cases:
#   - 'await' at module scope (or class scope): 'await' outside function
#   - 'await' inside a non-async function:      'await' outside async function

# --- Case 1: await at module scope ---

try:
    compile("await x", "<test>", "exec")
    print("ERROR: should have raised SyntaxError")
except SyntaxError as e:
    print("module scope ok:", "'await' outside function" in str(e))

# --- Case 2: await inside a non-async function ---

try:
    compile("def f():\n    await x\n", "<test>", "exec")
    print("ERROR: should have raised SyntaxError")
except SyntaxError as e:
    print("non-async function ok:", "'await' outside async function" in str(e))

# --- Case 3: dead assignment targets are still evaluated syntax contexts ---

for label, source, expected in (
    (
        "dead augassign await",
        "def f(items):\n    if False:\n        items[(await g())] += 1\n",
        "'await' outside async function",
    ),
    (
        "dead assign await",
        "def f(items):\n    if False:\n        items[(await g())] = 1\n",
        "'await' outside async function",
    ),
    (
        "dead augassign yield",
        "if False:\n    items[(yield 0)] += 1\n",
        "'yield' outside function",
    ),
):
    try:
        compile(source, "<test>", "exec")
        print(label, "ERROR: should have raised SyntaxError")
    except SyntaxError as e:
        print(label, expected in str(e))

# Evaluation order controls which context error wins when a dead statement
# contains more than one invalid expression.
for label, source, expected in (
    (
        "assign rhs first",
        "if False:\n    items[(yield 0)] = await g()\n",
        "'await' outside function",
    ),
    (
        "assign target second",
        "if False:\n    items[await g()] = (yield 0)\n",
        "'yield' outside function",
    ),
    (
        "decorator before default",
        "if False:\n    @(yield 1)\n    def f(value=await g()):\n        pass\n",
        "'yield' outside function",
    ),
):
    try:
        compile(source, "<test>", "exec")
        print(label, "ERROR: should have raised SyntaxError")
    except SyntaxError as e:
        print(label, expected in str(e))

# A class body is not made async by an enclosing async function.
for label, statement, expected in (
    ("dead class async for", "async for item in items:\n                pass", "'async for' outside async function"),
    ("dead class async with", "async with manager:\n                pass", "'async with' outside async function"),
):
    source = (
        "async def outer():\n"
        "    if False:\n"
        "        class Inner:\n"
        f"            {statement}\n"
    )
    try:
        compile(source, "<test>", "exec")
        print(label, "ERROR: should have raised SyntaxError")
    except SyntaxError as e:
        print(label, expected in str(e))
