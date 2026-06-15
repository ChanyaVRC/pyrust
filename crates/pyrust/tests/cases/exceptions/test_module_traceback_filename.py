# Issue #2438: a traceback frame must report the filename of the *code object's*
# source file (its co_filename), not the running script's path, and must carry
# the correct per-frame line number.
#
# Real imported-module tracebacks embed an absolute path that varies per machine,
# so this fixture uses `compile(src, "<name>", "exec")` to give a block of code a
# distinct, literal co_filename — exercising the same code-object-filename
# plumbing an imported module relies on — and then asserts STRUCTURE (filename,
# lineno, funcname) via the walkable __traceback__ chain.  Stable across machines.

# A function defined in code compiled under a synthetic filename: its __code__
# and every traceback frame for it must report that filename.
helper_src = """
def helper():
    raise ValueError("from helper")


def relay():
    helper()
"""

mod_ns = {}
exec(compile(helper_src, "synth_module.py", "exec"), mod_ns)
helper = mod_ns["helper"]
relay = mod_ns["relay"]

# co_filename comes from the code object, not the running script.
print("helper co_filename:", helper.__code__.co_filename)
print("relay co_filename:", relay.__code__.co_filename)
print("helper co_firstlineno:", helper.__code__.co_firstlineno)
print("relay co_firstlineno:", relay.__code__.co_firstlineno)

# An uncaught propagation through relay -> helper records each frame with the
# code object's own filename and the raising line number.
try:
    relay()
except ValueError as e:
    node = e.__traceback__
    rows = []
    while node is not None:
        code = node.tb_frame.f_code
        rows.append((code.co_filename, node.tb_lineno, code.co_name))
        node = node.tb_next
    print("depth:", len(rows))
    # The outermost `<module>` frame's filename is this running script's own path
    # (machine-dependent), so normalise it; the synthetic-module frames below it
    # are what this test asserts.
    for filename, lineno, name in rows:
        if name == "<module>":
            filename = "<script>"
        print(filename, lineno, name)

# A generator defined in the synthetic module reports the same filename when it
# raises mid-iteration.
gen_src = """
def counter():
    yield 1
    raise KeyError("from gen")
"""
gen_ns = {}
exec(compile(gen_src, "synth_gen.py", "exec"), gen_ns)
counter = gen_ns["counter"]
print("counter co_filename:", counter.__code__.co_filename)

try:
    for _ in counter():
        pass
except KeyError as e:
    node = e.__traceback__
    # Walk to the innermost frame (the generator body).
    last = None
    while node is not None:
        last = node
        node = node.tb_next
    code = last.tb_frame.f_code
    print("gen frame:", code.co_filename, last.tb_lineno, code.co_name)
