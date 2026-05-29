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
