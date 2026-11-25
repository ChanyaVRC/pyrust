# Comprehensive CPython 3.12 parity tests for `opcode` and `dis` (issue #3227).

import dis
import io
import opcode
import sys
import types


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


# ── 1. opcode module parity ──────────────────────────────────────────────────

print("opcode in sys.modules:", "opcode" in sys.modules)
print("opcode __all__:", opcode.__all__)
print("HAVE_ARGUMENT / EXTENDED_ARG:", opcode.HAVE_ARGUMENT, opcode.EXTENDED_ARG)
print("cmp_op:", opcode.cmp_op)
print("opname len:", len(opcode.opname))
print("opmap len:", len(opcode.opmap))
print("MIN_INSTRUMENTED / PSEUDO:", opcode.MIN_INSTRUMENTED_OPCODE, opcode.MIN_PSEUDO_OPCODE, opcode.MAX_PSEUDO_OPCODE)
print("is_pseudo:", opcode.is_pseudo(0), opcode.is_pseudo(255), opcode.is_pseudo(256), opcode.is_pseudo(266), opcode.is_pseudo(267))

# Every opmap entry maps back to opname identically
for name, code in sorted(opcode.opmap.items()):
    assert opcode.opname[code] == name, (name, code, opcode.opname[code])
print("opmap <-> opname roundtrip ok")

print("hasconst:", opcode.hasconst)
print("hasname:", opcode.hasname)
print("hasjrel:", opcode.hasjrel)
print("hasjabs:", opcode.hasjabs)
print("haslocal:", opcode.haslocal)
print("hascompare:", opcode.hascompare)
print("hasfree:", opcode.hasfree)
print("hasexc:", opcode.hasexc)
print("hasarg len:", len(opcode.hasarg), opcode.opmap["LOAD_CONST"] in opcode.hasarg, opcode.opmap["POP_TOP"] in opcode.hasarg)

# Exhaustive stack_effect check across all opcodes in opmap
se_results = []
for name, op in sorted(opcode.opmap.items(), key=lambda kv: kv[1]):
    if op == opcode.opmap["INSTRUMENTED_LINE"]:
        se_results.append((op, expect_exc(opcode.stack_effect, op, 0)[0]))
    elif op < opcode.HAVE_ARGUMENT or op in (256, 257, 258, 259):
        se_results.append((
            op,
            opcode.stack_effect(op),
            opcode.stack_effect(op, jump=True),
            opcode.stack_effect(op, jump=False),
        ))
    else:
        se_results.append((
            op,
            opcode.stack_effect(op, 0),
            opcode.stack_effect(op, 1),
            opcode.stack_effect(op, 2),
            opcode.stack_effect(op, 3),
            opcode.stack_effect(op, 4),
            opcode.stack_effect(op, 257),
        ))
print("stack_effect count:", len(se_results))
print("stack_effect sample:", se_results[:10], se_results[-8:])
# Verify exact full checksum of all stack_effect values
total_se = 0
for row in se_results:
    for item in row:
        if isinstance(item, int):
            total_se += item
print("stack_effect checksum:", total_se)

# stack_effect error cases
print("se missing oparg:", expect_exc(opcode.stack_effect, opcode.opmap["LOAD_CONST"]))
print("se unexpected oparg:", expect_exc(opcode.stack_effect, opcode.opmap["POP_TOP"], 0))
print("se invalid jump:", expect_exc(opcode.stack_effect, opcode.opmap["LOAD_CONST"], 0, jump="bad"))
print("se invalid op:", expect_exc(opcode.stack_effect, -1))
print("se unused op:", expect_exc(opcode.stack_effect, 8))
print("se bad type:", expect_exc(opcode.stack_effect, "LOAD_CONST")[0])


# ── 2. dis module exports & constants ────────────────────────────────────────

print("dis in sys.modules:", "dis" in sys.modules)
print("dis __all__:", dis.__all__)
print("dis re-exports opcode:",
      dis.opmap is opcode.opmap,
      dis.opname is opcode.opname,
      dis.HAVE_ARGUMENT == opcode.HAVE_ARGUMENT,
      dis.EXTENDED_ARG == opcode.EXTENDED_ARG,
      dis.cmp_op == opcode.cmp_op,
      dis.stack_effect is opcode.stack_effect)
print("disco is disassemble:", dis.disco is dis.disassemble)
print("COMPILER_FLAG_NAMES:", dis.COMPILER_FLAG_NAMES)

# pretty_flags
for fl in (0, 1, 3, 67, 1 | 2 | 4 | 8, 32, 128, 256, 512, 0x10000, 3 | 0x10000):
    print(f"pretty_flags({fl}):", dis.pretty_flags(fl))


# ── 3. Positions & Instruction namedtuples ───────────────────────────────────

p_def = dis.Positions()
p_full = dis.Positions(10, 10, 4, 12)
print("Positions default:", p_def, p_def.lineno, p_def.end_lineno, p_def.col_offset, p_def.end_col_offset)
print("Positions full:", p_full, tuple(p_full))

ins1 = dis.Instruction("LOAD_CONST", 100, 1, 42, "42", 0, 5, False)
ins2 = dis.Instruction("RETURN_VALUE", 83, None, None, "", 2, None, True, p_full)
print("Instruction defaults:", ins1.positions, ins1.opname, ins1.opcode, ins1.arg, ins1.argval, ins1.argrepr, ins1.offset, ins1.starts_line, ins1.is_jump_target)
print("Instruction _disassemble 1:", repr(ins1._disassemble()))
print("Instruction _disassemble 2:", repr(ins2._disassemble(mark_as_current=True)))
print("Instruction _disassemble no lineno:", repr(ins1._disassemble(lineno_width=0)))


# ── 4. findlabels on raw CPython 3.12 bytecode bytes ─────────────────────────

raw_bc = bytes([
    opcode.opmap["LOAD_CONST"], 0,          # offset 0
    opcode.opmap["POP_JUMP_IF_FALSE"], 2,   # offset 2 -> target 2 + 2 + 4 = 8
    opcode.opmap["LOAD_CONST"], 1,          # offset 4
    opcode.opmap["JUMP_FORWARD"], 1,        # offset 6 -> target 6 + 2 + 2 = 10
    opcode.opmap["LOAD_CONST"], 2,          # offset 8
    opcode.opmap["RETURN_VALUE"], 0,        # offset 10
])
print("findlabels raw:", dis.findlabels(raw_bc))

# Disassemble raw bytecode bytes
raw_buf = io.StringIO()
dis.dis(raw_bc, file=raw_buf)
print("dis raw_bc lines:", len(raw_buf.getvalue().strip().splitlines()))
print("dis raw_bc has >>:", ">>" in raw_buf.getvalue())
print("dis raw_bc has JUMP_FORWARD:", "JUMP_FORWARD" in raw_buf.getvalue())


# ── 5. code_info & show_code ─────────────────────────────────────────────────

def sample_fn(a, /, b, *, c=1, **kw):
    total = a + b + c
    for i in range(3):
        if i % 2 == 0:
            total += i
    return total


def outer_fn(x):
    def inner_fn(y):
        return x + y
    return inner_fn


def gen_fn(n):
    for i in range(n):
        yield i


async def coro_fn(x):
    return x * 2


class SampleClass:
    def method(self, v):
        return v + 1

    @staticmethod
    def smethod(v):
        return v + 2

    @classmethod
    def cmethod(cls, v):
        return v + 3


info_str = dis.code_info(sample_fn)
info_lines = info_str.splitlines()
for prefix in (
    "Name:              sample_fn",
    "Argument count:    2",
    "Positional-only arguments: 1",
    "Kw-only arguments: 1",
    "Number of locals:  6",
    "Variable names:",
):
    assert prefix in info_str, f"missing {prefix!r} in:\n{info_str}"
assert any(l.startswith("Filename:") for l in info_lines)
assert any(l.startswith("Stack size:") for l in info_lines)
assert any(l.startswith("Flags:") and "OPTIMIZED" in l and "NEWLOCALS" in l and "VARKEYWORDS" in l for l in info_lines)
print("code_info sample_fn ok")

# Closure cellvars & freevars in code_info
outer_info = dis.code_info(outer_fn)
inner_info = dis.code_info(outer_fn(10))
assert "Cell variables:" in outer_info and "x" in outer_info
assert "Free variables:" in inner_info and "x" in inner_info
print("code_info closure ok")

# show_code writes code_info to file
sc_buf = io.StringIO()
dis.show_code(sample_fn, file=sc_buf)
print("show_code matches code_info:", sc_buf.getvalue() == info_str + "\n")

# code_info on bound method, generator, coroutine, string
assert "Name:              method" in dis.code_info(SampleClass().method)
assert "Name:              gen_fn" in dis.code_info(gen_fn(3))
assert "GENERATOR" in dis.code_info(gen_fn(3))
c_obj = coro_fn(5)
assert "Name:              coro_fn" in dis.code_info(c_obj)
assert "COROUTINE" in dis.code_info(c_obj)
c_obj.close()
assert "Name:              <module>" in dis.code_info("x = 1 + 2")
print("code_info variants ok")


# ── 6. get_instructions, findlinestarts, Bytecode, dis, disassemble ──────────

insns = list(dis.get_instructions(sample_fn))
assert len(insns) > 0
assert all(isinstance(i, dis.Instruction) for i in insns)
assert all(isinstance(i.opname, str) and len(i.opname) > 0 for i in insns)
assert all(isinstance(i.opcode, int) for i in insns)
assert all(isinstance(i.argrepr, str) for i in insns)
assert all(isinstance(i.offset, int) and i.offset >= 0 for i in insns)
assert all(insns[k].offset < insns[k + 1].offset for k in range(len(insns) - 1))
assert all(isinstance(i.is_jump_target, bool) for i in insns)
assert all(isinstance(i.positions, dis.Positions) for i in insns)
assert any(i.is_jump_target for i in insns), "loop/if function must have jump targets"
assert any(i.starts_line is not None for i in insns), "function must have starts_line"
print("get_instructions sample_fn ok")

# first_line shifting
insns_shifted = list(dis.get_instructions(sample_fn, first_line=100))
first_orig_line = next(i.starts_line for i in insns if i.starts_line is not None)
first_shift_line = next(i.starts_line for i in insns_shifted if i.starts_line is not None)
print("first_line shift delta:", first_shift_line - first_orig_line == 100 - sample_fn.__code__.co_firstlineno)

# findlinestarts
ls = list(dis.findlinestarts(sample_fn.__code__))
assert len(ls) > 0
assert all(isinstance(off, int) and isinstance(ln, int) for off, ln in ls)
assert all(ls[k][0] < ls[k + 1][0] for k in range(len(ls) - 1))
print("findlinestarts ok")

# Bytecode API
bc = dis.Bytecode(sample_fn)
print("Bytecode codeobj:", bc.codeobj is sample_fn.__code__)
print("Bytecode first_line:", bc.first_line == sample_fn.__code__.co_firstlineno)
print("Bytecode current_offset:", bc.current_offset)
print("Bytecode info eq:", bc.info() == dis.code_info(sample_fn))
print("Bytecode iter eq:", list(bc) == insns)
bc_dis = bc.dis()
assert isinstance(bc_dis, str) and len(bc_dis.strip().splitlines()) >= len(insns)
print("Bytecode dis non-empty:", True)

# Bytecode with current_offset highlights instruction with '-->'
bc_cur = dis.Bytecode(sample_fn, current_offset=insns[0].offset)
print("Bytecode current_offset marker:", "-->" in bc_cur.dis())

# Bytecode from string source
bc_str = dis.Bytecode("a = 1 + 2")
assert len(list(bc_str)) > 0
assert len(bc_str.dis().strip()) > 0
print("Bytecode from str ok")

# dis.dis() on various objects
for label, obj in [
    ("function", sample_fn),
    ("method", SampleClass().method),
    ("staticmethod", SampleClass.smethod),
    ("classmethod", SampleClass.cmethod),
    ("code", sample_fn.__code__),
    ("expr_str", "1 + 2"),
    ("stmt_str", "x = 1\ny = x + 2"),
    ("compiled", compile("z = 3 * 4", "<test>", "exec")),
]:
    buf = io.StringIO()
    dis.dis(obj, file=buf)
    assert len(buf.getvalue().strip()) > 0, f"empty dis for {label}"
print("dis() on targets ok")

# dis() on a class disassembles its methods
cls_buf = io.StringIO()
dis.dis(SampleClass, file=cls_buf)
cls_out = cls_buf.getvalue()
assert "Disassembly of cmethod:" in cls_out
assert "Disassembly of method:" in cls_out
assert "Disassembly of smethod:" in cls_out
print("dis(class) ok")

# dis() recursive depth control
rec_all = io.StringIO()
dis.dis(outer_fn, file=rec_all)
rec_0 = io.StringIO()
dis.dis(outer_fn, file=rec_0, depth=0)
assert "Disassembly of <code object inner_fn" in rec_all.getvalue()
assert "Disassembly of <code object inner_fn" not in rec_0.getvalue()
print("dis(depth) ok")

# disassemble() directly on code object
d_buf = io.StringIO()
dis.disassemble(sample_fn.__code__, file=d_buf)
assert len(d_buf.getvalue().strip()) > 0
print("disassemble() ok")

# TypeError on non-disassemblable object
print("dis(123):", expect_exc(dis.dis, 123))
print("code_info(123):", expect_exc(dis.code_info, 123))
print("Bytecode(123):", expect_exc(dis.Bytecode, 123))


# ── 7. distb & Bytecode.from_traceback ───────────────────────────────────────

def boom():
    return 1 / 0


def caller():
    try:
        return boom()
    except ZeroDivisionError as exc:
        return exc.__traceback__


tb = caller()

bc_tb = dis.Bytecode.from_traceback(tb)
print("from_traceback code name:", bc_tb.codeobj.co_name)
assert len(list(bc_tb)) > 0
tb_buf = io.StringIO()
dis.distb(tb, file=tb_buf)
assert len(tb_buf.getvalue().strip()) > 0
print("distb(tb) ok")

# distb(None) / dis() with no last traceback raises RuntimeError
saved_last_tb = getattr(sys, "last_traceback", None)
saved_last_exc = getattr(sys, "last_exc", None)
if hasattr(sys, "last_traceback"):
    delattr(sys, "last_traceback")
if hasattr(sys, "last_exc"):
    delattr(sys, "last_exc")
try:
    print("distb no last:", expect_exc(dis.distb))
    print("dis() no last:", expect_exc(dis.dis))
    sys.last_traceback = tb
    last_buf = io.StringIO()
    dis.dis(file=last_buf)
    assert len(last_buf.getvalue().strip()) > 0
    print("dis() with sys.last_traceback ok")
finally:
    if hasattr(sys, "last_traceback"):
        delattr(sys, "last_traceback")
    if saved_last_tb is not None:
        sys.last_traceback = saved_last_tb
    if saved_last_exc is not None:
        sys.last_exc = saved_last_exc
