# CPython 3.12 parity tests for issue #3242:
# 8-column tab indentation + TabError, and invalid-escape SyntaxWarning.

import warnings

# 1. Valid tab-indented Python code (including 8-column tab expansion)
tab_src = (
    "def compute(n):\n"
    "\ttotal = 0\n"
    "\tfor i in range(n):\n"
    "\t\tif i % 2 == 0:\n"
    "\t\t\ttotal += i\n"
    "\treturn total\n"
    "res = compute(10)\n"
)
ns = {}
exec(tab_src, ns)
print("tab valid:", ns["res"])

# Valid: 1 tab (col 8, altcol 1) followed by 1 tab + 4 spaces (col 12, altcol 5)
tab_plus_spaces = (
    "def f():\n"
    "\tif True:\n"
    "\t    return 42\n"
    "out = f()\n"
)
ns2 = {}
exec(tab_plus_spaces, ns2)
print("tab then tab+spaces:", ns2["out"])

# Blank lines with tabs do not affect indentation
blank_tab_src = (
    "def g():\n"
    "\t\t   \n"
    "    return 7\n"
    "out = g()\n"
)
ns3 = {}
exec(blank_tab_src, ns3)
print("blank tab line:", ns3["out"])

# 2. Mixed tabs and spaces raising TabError (subclass of IndentationError and SyntaxError)
print("TabError hierarchy:", issubclass(TabError, IndentationError), issubclass(TabError, SyntaxError))

mixed_cases = [
    ("tab_vs_8spaces", "if True:\n\tx = 1\n        y = 2\n"),
    ("4spaces_then_tab", "if True:\n    x = 1\n\ty = 2\n"),
    ("8spaces_then_tab4", "if True:\n        if True:\n\t    x = 1\n"),
]
for name, src in mixed_cases:
    try:
        compile(src, "<tabtest>", "exec")
        print(f"mixed {name}: NO ERROR")
    except TabError as e:
        print(f"mixed {name}: TabError", "inconsistent use of tabs and spaces" in str(e))

# 3. Invalid escape SyntaxWarning
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("always", SyntaxWarning)
    val_s = eval(compile("'\\d+'", "<esc1>", "eval"))
    val_b = eval(compile("b'\\w+'", "<esc2>", "eval"))
    val_raw = eval(compile("r'\\d+'", "<esc3>", "eval"))
    val_oct = eval(compile("b'\\400'", "<esc4>", "eval"))
    print("values:", val_s, val_b, val_raw, val_oct)
    print("warning count:", len(w))
    print("all SyntaxWarning:", all(item.category is SyntaxWarning for item in w))
    print("messages mention invalid:", all("invalid" in str(item.message) and "escape sequence" in str(item.message) for item in w))

# filterwarnings('ignore') suppresses SyntaxWarning
with warnings.catch_warnings(record=True) as w_ign:
    warnings.filterwarnings("ignore", category=SyntaxWarning)
    compile("'\\d'", "<ign>", "eval")
    print("ignored count:", len(w_ign))

# filterwarnings('error', category=SyntaxWarning) raises SyntaxError (CPython 3.12+)
with warnings.catch_warnings():
    warnings.filterwarnings("error", category=SyntaxWarning)
    try:
        compile("'\\d'", "<err>", "eval")
        print("error mode: NO ERROR")
    except SyntaxError as e:
        print("error mode:", type(e).__name__, "invalid" in str(e))
