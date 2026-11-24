# CPython 3.12 parity tests for `argparse` module (issue #3150).
import argparse
import contextlib
import io
import os
import sys
import warnings

# 1. Module surface & constants
print("version:", argparse.__version__)
print("all:", argparse.__all__)
print("constants:", argparse.ONE_OR_MORE, argparse.OPTIONAL, argparse.PARSER,
      argparse.REMAINDER, argparse.SUPPRESS, argparse.ZERO_OR_MORE)
for cls_name in (
    "ArgumentParser",
    "ArgumentError",
    "ArgumentTypeError",
    "BooleanOptionalAction",
    "FileType",
    "HelpFormatter",
    "ArgumentDefaultsHelpFormatter",
    "RawDescriptionHelpFormatter",
    "RawTextHelpFormatter",
    "MetavarTypeHelpFormatter",
    "Namespace",
    "Action",
):
    print("module", cls_name, getattr(argparse, cls_name).__module__)

# 2. Namespace
ns1 = argparse.Namespace(a=1, b="two")
ns2 = argparse.Namespace(a=1, b="two")
ns3 = argparse.Namespace(a=1, b="three")
print("ns repr:", repr(ns1))
print("ns eq:", ns1 == ns2, ns1 != ns3, ns1 == {"a": 1, "b": "two"})
print("ns contains:", "a" in ns1, "c" in ns1)
setattr(ns1, "non-ident", 99)
print("ns star repr:", repr(ns1))

# 3. Basic positional & optional arguments, flag forms, clustering, --
p = argparse.ArgumentParser(prog="demo")
p.add_argument("pos1")
p.add_argument("pos2", type=int)
p.add_argument("-a", action="store_true")
p.add_argument("-b", action="store_true")
p.add_argument("-c", action="store_false")
p.add_argument("-n", "--num", type=int, default=5)
p.add_argument("-s", "--str-opt", dest="sopt")
p.add_argument("-v", "--verbose", action="count", default=0)

res = p.parse_args(["hello", "42", "-ab", "-n10", "--str-opt=world", "-vvv"])
print("basic:", res)
res2 = p.parse_args(["-c", "--", "-not-a-flag", "-7"])
print("terminator & neg int:", res2)
print("default prog matches basename:", argparse.ArgumentParser().prog == os.path.basename(sys.argv[0]))
print("parser repr:", repr(p))

# 4. All built-in actions + BooleanOptionalAction + custom Action
p_act = argparse.ArgumentParser(prog="acts")
p_act.add_argument("--store", action="store")
p_act.add_argument("--const", action="store_const", const=123, default=0)
p_act.add_argument("--true", action="store_true")
p_act.add_argument("--false", action="store_false")
p_act.add_argument("--app", action="append")
p_act.add_argument("--app-def", action="append", default=["init"])
p_act.add_argument("--appc", action="append_const", const="X")
p_act.add_argument("--appc2", dest="appc", action="append_const", const="Y")
p_act.add_argument("--cnt", action="count")
p_act.add_argument("--ext", action="extend", nargs="+", type=int)
p_act.add_argument("--flag", "-f", action=argparse.BooleanOptionalAction, default=None)
p_act.add_argument("--ver", action="version", version="%(prog)s 2.5")

class CustomAction(argparse.Action):
    def __call__(self, parser, namespace, values, option_string=None):
        setattr(namespace, self.dest, (option_string, values.upper()))

p_act.add_argument("--custom", action=CustomAction)

print("acts default:", p_act.parse_args([]))
print("acts full:", p_act.parse_args([
    "--store", "val",
    "--const",
    "--true",
    "--false",
    "--app", "1", "--app", "2",
    "--app-def", "next",
    "--appc", "--appc2",
    "--cnt", "--cnt",
    "--ext", "1", "2", "--ext", "3",
    "--flag",
    "--custom", "abc",
]))
print("acts no-flag:", p_act.parse_args(["--no-flag"]).flag)
print("acts short flag:", p_act.parse_args(["-f"]).flag)
# Ensure default list wasn't mutated across parses
print("acts second default:", p_act.parse_args([]).app_def)

# Version action output & exit code 0
buf_out = io.StringIO()
with contextlib.redirect_stdout(buf_out):
    try:
        p_act.parse_args(["--ver"])
    except SystemExit as e:
        print("version exit code:", e.code)
print("version output:", repr(buf_out.getvalue()))

# Help action output & exit code 0
buf_out = io.StringIO()
with contextlib.redirect_stdout(buf_out):
    try:
        p_act.parse_args(["-h"])
    except SystemExit as e:
        print("help exit code:", e.code)
print("help output has BooleanOptionalAction:", "--flag, --no-flag" in buf_out.getvalue())
print("help output matches format_help:", buf_out.getvalue() == p_act.format_help())

# Action.__call__ raises NotImplementedError
try:
    argparse.Action([], "x")(p_act, argparse.Namespace(), "v")
except NotImplementedError as e:
    print("base Action call:", str(e))

# BooleanOptionalAction deprecated arguments warning
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("always")
    p_warn = argparse.ArgumentParser(prog="warn")
    p_warn.add_argument("--opt", action=argparse.BooleanOptionalAction, type=int)
    print("bool opt warning:", [(str(x.message), x.category.__name__) for x in w])

# 5. nargs variations (?, *, +, int, REMAINDER) & SUPPRESS
p_nargs = argparse.ArgumentParser(prog="nargs")
p_nargs.add_argument("--opt", nargs="?", const="CONST", default="DEF")
p_nargs.add_argument("--pair", nargs=2, type=int, metavar=("X", "Y"))
p_nargs.add_argument("--star", nargs="*")
p_nargs.add_argument("--plus", nargs="+")
p_nargs.add_argument("--hidden", default=argparse.SUPPRESS, help=argparse.SUPPRESS)
p_nargs.add_argument("p_opt", nargs="?", default="42", type=int)
p_nargs.add_argument("rest", nargs=argparse.REMAINDER)

print("nargs empty:", p_nargs.parse_args([]))
print("nargs opt const:", p_nargs.parse_args(["--opt"]))
print("nargs opt val:", p_nargs.parse_args(["--opt", "VAL", "--pair", "3", "4", "--star", "a", "b", "--plus", "z"]))
print("nargs remainder:", p_nargs.parse_args(["10", "--unknown", "-x", "foo"]))
print("nargs hidden absent:", "hidden" in p_nargs.parse_args([]))
print("nargs hidden present:", p_nargs.parse_args(["--hidden", "seen"]).hidden)
print("nargs usage:", repr(p_nargs.format_usage()))

# 6. type, choices, required, custom ArgumentTypeError, register
def positive_int(s):
    v = int(s)
    if v <= 0:
        raise argparse.ArgumentTypeError(f"{s} is not a positive int")
    return v

p_tc = argparse.ArgumentParser(prog="tc", exit_on_error=False)
p_tc.register("type", "upper", lambda s: s.upper())
p_tc.add_argument("--count", type=positive_int, required=True)
p_tc.add_argument("--color", choices=["red", "green", "blue"], default="red")
p_tc.add_argument("--tag", type="upper")
print("tc ok:", p_tc.parse_args(["--count", "5", "--color", "blue", "--tag", "hello"]))

for bad in (
    ["--count", "0"],
    ["--count", "abc"],
    ["--count", "1", "--color", "yellow"],
):
    try:
        p_tc.parse_args(bad)
    except argparse.ArgumentError as e:
        print("tc ArgumentError:", str(e))

# 7. parse_known_args, parse_intermixed_args, parse_known_intermixed_args
p_k = argparse.ArgumentParser(prog="known")
p_k.add_argument("--foo", action="store_true")
p_k.add_argument("bar")
print("known:", p_k.parse_known_args(["--foo", "--bad", "BAR", "extra"]))

p_mix = argparse.ArgumentParser(prog="mix")
p_mix.add_argument("--foo")
p_mix.add_argument("cmd")
p_mix.add_argument("rest", nargs="*", type=int)
print("intermixed:", p_mix.parse_intermixed_args(["doit", "1", "--foo", "bar", "2", "3"]))
print("known intermixed:", p_mix.parse_known_intermixed_args(["doit", "1", "--foo", "bar", "--extra", "2"]))

# Intermixed rejects REMAINDER
p_rem = argparse.ArgumentParser(prog="rem")
p_rem.add_argument("rest", nargs=argparse.REMAINDER)
try:
    p_rem.parse_intermixed_args([])
except TypeError as e:
    print("intermixed remainder TypeError:", str(e))

# 8. set_defaults, get_default, custom namespace, allow_abbrev, prefix_chars
p_def = argparse.ArgumentParser(prog="defs", argument_default=argparse.SUPPRESS)
p_def.add_argument("--foo")
p_def.add_argument("--bar", default=10)
p_def.set_defaults(extra="preset", bar=20)
print("get_default:", p_def.get_default("bar"), p_def.get_default("extra"), p_def.get_default("missing"))
print("defs parse:", p_def.parse_args([]))

custom_ns = argparse.Namespace(existing=123)
p_ns = argparse.ArgumentParser(prog="cns")
p_ns.add_argument("--x", type=int)
out_ns = p_ns.parse_args(["--x", "7"], namespace=custom_ns)
print("custom ns same obj:", out_ns is custom_ns, out_ns)

p_abbr_on = argparse.ArgumentParser(prog="abbr", allow_abbrev=True)
p_abbr_on.add_argument("--foobar")
p_abbr_on.add_argument("--foobaz")
p_abbr_on.add_argument("--quux")
print("abbrev on:", p_abbr_on.parse_args(["--qu", "1"]))

p_abbr_off = argparse.ArgumentParser(prog="abbr", allow_abbrev=False)
p_abbr_off.add_argument("--quux")
print("abbrev off known:", p_abbr_off.parse_known_args(["--qu", "1"]))

p_plus = argparse.ArgumentParser(prog="plus", prefix_chars="-+")
p_plus.add_argument("+f", "++foo")
p_plus.add_argument("-b", "--bar", action="store_true")
print("prefix_chars:", p_plus.parse_args(["+f", "hi", "-b"]))

# 9. Argument groups & mutually exclusive groups
p_grp = argparse.ArgumentParser(prog="grp", description="A sample program.", epilog="The end.")
g1 = p_grp.add_argument_group("input options", "Options controlling input.")
g1.add_argument("-i", "--input", help="input path")
mx = p_grp.add_mutually_exclusive_group(required=True)
mx.add_argument("--json", action="store_true", help="output JSON")
mx.add_argument("--xml", action="store_true", help="output XML")
print("mx json:", p_grp.parse_args(["--json"]))
print("mx xml:", p_grp.parse_args(["--xml"]))

# Mutually exclusive conflict with exit_on_error=False
p_mx_err = argparse.ArgumentParser(prog="mxerr", exit_on_error=False)
mx2 = p_mx_err.add_mutually_exclusive_group()
mx2.add_argument("-x", action="store_true")
mx2.add_argument("-y", action="store_true")
try:
    p_mx_err.parse_args(["-x", "-y"])
except argparse.ArgumentError as e:
    print("mx conflict:", str(e))

# Mutually exclusive required missing (stderr + SystemExit(2))
buf_err = io.StringIO()
with contextlib.redirect_stderr(buf_err):
    try:
        p_grp.parse_args([])
    except SystemExit as e:
        print("mx required exit:", e.code)
print("mx required stderr:", repr(buf_err.getvalue()))

# 10. Subparsers with aliases, dest, required, defaults
p_sub = argparse.ArgumentParser(prog="tool")
p_sub.add_argument("--global-flag", action="store_true")
subparsers = p_sub.add_subparsers(title="subcommands", description="valid commands",
                                  dest="command", required=True, help="command help")
p_run = subparsers.add_parser("run", aliases=["r", "exec"], help="run a target")
p_run.add_argument("target")
p_run.add_argument("--dry-run", action="store_true")
p_run.set_defaults(mode="runner")

p_build = subparsers.add_parser("build", aliases=["b"], help="build project")
p_build.add_argument("-j", "--jobs", type=int, default=1)

print("sub run:", p_sub.parse_args(["--global-flag", "run", "app", "--dry-run"]))
print("sub alias:", p_sub.parse_args(["r", "app2"]))
print("sub build:", p_sub.parse_args(["b", "-j4"]))
print("sub known:", p_sub.parse_known_args(["b", "--unknown", "extra"]))
print("sub help:\n" + p_sub.format_help())
print("sub run help:\n" + p_run.format_help())

# Conflicting subparser / alias
try:
    subparsers.add_parser("run")
except argparse.ArgumentError as e:
    print("sub conflict:", str(e))
try:
    subparsers.add_parser("other", aliases=["b"])
except argparse.ArgumentError as e:
    print("sub alias conflict:", str(e))

# 11. parents & conflict_handler
parent = argparse.ArgumentParser(add_help=False)
parent.add_argument("--parent-opt", type=int, default=7)
parent.set_defaults(from_parent=True)

child = argparse.ArgumentParser(prog="child", parents=[parent])
child.add_argument("--child-opt")
print("parents:", child.parse_args(["--parent-opt", "9", "--child-opt", "ok"]))

p_res = argparse.ArgumentParser(prog="res", conflict_handler="resolve")
p_res.add_argument("-f", "--foo", help="old foo")
p_res.add_argument("--foo", help="new foo", default="NEW")
print("conflict resolve:", p_res.parse_args(["--foo", "v"]), p_res.parse_args(["-f", "old"]))

p_err = argparse.ArgumentParser(prog="err", conflict_handler="error")
p_err.add_argument("-f", "--foo")
try:
    p_err.add_argument("--foo")
except argparse.ArgumentError as e:
    print("conflict error:", str(e))

# 12. fromfile_prefix_chars & convert_arg_line_to_args
tmp1 = "_pyrust_argparse_test_args1.txt"
tmp2 = "_pyrust_argparse_test_args2.txt"
try:
    with open(tmp2, "w", encoding="utf-8") as f:
        f.write("--nested\n99\n")
    with open(tmp1, "w", encoding="utf-8") as f:
        f.write("--name\nalice\n@" + tmp2 + "\n")

    p_file = argparse.ArgumentParser(prog="fromfile", fromfile_prefix_chars="@")
    p_file.add_argument("--name")
    p_file.add_argument("--nested", type=int)
    print("fromfile:", p_file.parse_args(["@" + tmp1]))

    class SplitLineParser(argparse.ArgumentParser):
        def convert_arg_line_to_args(self, arg_line):
            return arg_line.split()

    with open(tmp1, "w", encoding="utf-8") as f:
        f.write("--name bob --nested 123\n")
    p_split = SplitLineParser(prog="split", fromfile_prefix_chars="@")
    p_split.add_argument("--name")
    p_split.add_argument("--nested", type=int)
    print("fromfile split:", p_split.parse_args(["@" + tmp1]))
finally:
    for path in (tmp1, tmp2):
        if os.path.exists(path):
            os.remove(path)

# 13. FileType
print("FileType reprs:",
      repr(argparse.FileType()),
      repr(argparse.FileType("w", encoding="utf-8")),
      repr(argparse.FileType("r", 0, errors="ignore")))
ft_r = argparse.FileType("r")
ft_w = argparse.FileType("w")
print("FileType dash:", ft_r("-") is sys.stdin, ft_w("-") is sys.stdout)

tmp_ft = "_pyrust_argparse_filetype_test.txt"
try:
    p_ft = argparse.ArgumentParser(prog="ft")
    p_ft.add_argument("out", type=argparse.FileType("w", encoding="utf-8"))
    ns_w = p_ft.parse_args([tmp_ft])
    ns_w.out.write("hello filetype\n")
    ns_w.out.close()

    p_ft_r = argparse.ArgumentParser(prog="ftr")
    p_ft_r.add_argument("inp", type=argparse.FileType("r", encoding="utf-8"))
    ns_r = p_ft_r.parse_args([tmp_ft])
    print("FileType read:", repr(ns_r.inp.read()))
    ns_r.inp.close()
finally:
    if os.path.exists(tmp_ft):
        os.remove(tmp_ft)

try:
    p_bad_ft = argparse.ArgumentParser(prog="badft")
    p_bad_ft.add_argument("f", type=argparse.FileType)
except ValueError as e:
    print("FileType class error:", str(e))

try:
    argparse.FileType("r")("_pyrust_nonexistent_file_99999.txt")
except argparse.ArgumentTypeError as e:
    print("FileType missing starts with:", str(e).startswith("can't open '_pyrust_nonexistent_file_99999.txt':"))

# 14. HelpFormatters (HelpFormatter, RawDescription, RawText, ArgumentDefaults, MetavarType)
desc = "Line one.\n    Indented line two.\n\nParagraph two."
for fmt_cls in (
    argparse.HelpFormatter,
    argparse.RawDescriptionHelpFormatter,
    argparse.RawTextHelpFormatter,
    argparse.ArgumentDefaultsHelpFormatter,
    argparse.MetavarTypeHelpFormatter,
):
    pf = argparse.ArgumentParser(
        prog="fmt",
        description=desc,
        epilog="Epilog:\n  keep indent",
        formatter_class=fmt_cls,
    )
    if fmt_cls is argparse.MetavarTypeHelpFormatter:
        pf.add_argument("--num", type=int, default=3, help="a number\n  second line")
        pf.add_argument("val", type=float, help="a float")
    else:
        pf.add_argument("--num", type=int, default=3, help="a number\n  second line")
        pf.add_argument("val", help="a value")
    print(f"=== {fmt_cls.__name__} ===")
    print(pf.format_help())

# Long usage wrapping with COLUMNS
saved_cols = os.environ.get("COLUMNS")
try:
    os.environ["COLUMNS"] = "50"
    p_wrap = argparse.ArgumentParser(prog="longprogram")
    for idx in range(6):
        p_wrap.add_argument(f"--option-number-{idx}", metavar=f"VAL{idx}", help=f"help {idx}")
    p_wrap.add_argument("first_pos")
    p_wrap.add_argument("second_pos")
    print("=== wrapped usage ===")
    print(p_wrap.format_usage())
finally:
    if saved_cols is None:
        if "COLUMNS" in os.environ:
            del os.environ["COLUMNS"]
    else:
        os.environ["COLUMNS"] = saved_cols

# 15. print_usage / print_help to custom file
s_u = io.StringIO()
s_h = io.StringIO()
p_grp.print_usage(s_u)
p_grp.print_help(s_h)
print("print_usage eq:", s_u.getvalue() == p_grp.format_usage())
print("print_help eq:", s_h.getvalue() == p_grp.format_help())

# 16. Negative number options vs negative number arguments
p_neg = argparse.ArgumentParser(prog="neg")
p_neg.add_argument("-1", dest="one", action="store_true")
p_neg.add_argument("x", nargs="?")
print("neg opt:", p_neg.parse_args(["-1"]))

# 17. Unrecognized args & ambiguous option stderr
buf_err = io.StringIO()
with contextlib.redirect_stderr(buf_err):
    try:
        p_abbr_on.parse_args(["--foo", "1"])
    except SystemExit as e:
        print("ambiguous exit:", e.code)
print("ambiguous stderr:", repr(buf_err.getvalue()))

# 18. Custom usage & percent escaping in help/usage
p_pct = argparse.ArgumentParser(prog="pct", usage="%(prog)s [100%% custom]")
p_pct.add_argument("--rate", default=50, help="rate is 100%% (default %(default)s, dest %(dest)s)")
p_pct.add_argument("--plain", help="no placeholders at all")
print("pct usage:", repr(p_pct.format_usage()))
print("pct help:\n" + p_pct.format_help())

p_plain_u = argparse.ArgumentParser(prog="pu", usage="literal usage 50%%")
print("plain usage:", repr(p_plain_u.format_usage()))

# 19. add_argument & parser validation errors
p_val = argparse.ArgumentParser(prog="val")
for label, fn in (
    ("pos required", lambda: p_val.add_argument("x", required=True)),
    ("pos dest twice", lambda: p_val.add_argument("x", dest="y")),
    ("bad opt prefix", lambda: p_val.add_argument("-a", "b")),
    ("empty dest", lambda: p_val.add_argument("--")),
    ("store nargs 0", lambda: p_val.add_argument("--z", action="store", nargs=0)),
    ("store const bad nargs", lambda: p_val.add_argument("--z", action="store", const=1)),
    ("append nargs 0", lambda: p_val.add_argument("--z", action="append", nargs=0)),
    ("unknown action", lambda: p_val.add_argument("--z", action="not_an_action")),
    ("non-callable type", lambda: p_val.add_argument("--z", type=123)),
    ("bad metavar tuple", lambda: p_val.add_argument("--z", nargs=2, metavar=("A",))),
    ("bad conflict_handler", lambda: argparse.ArgumentParser(conflict_handler="nope")),
):
    try:
        fn()
    except Exception as e:
        print("val err", label, type(e).__name__, str(e))

mx_bad = p_val.add_mutually_exclusive_group()
try:
    mx_bad.add_argument("--req", required=True)
except ValueError as e:
    print("mx required arg err:", str(e))

# 20. exit_on_error=False nargs / explicit-arg errors & ArgumentError(None, ...)
p_eoe = argparse.ArgumentParser(prog="eoe", exit_on_error=False)
p_eoe.add_argument("-a", action="store_true")
p_eoe.add_argument("--one", nargs=1)
p_eoe.add_argument("--two", nargs=2)
p_eoe.add_argument("--plus", nargs="+")
for bad in (
    ["--one"],
    ["--two", "x"],
    ["--plus"],
    ["-a=extra"],
    ["--one=a", "--two=b"],
):
    try:
        p_eoe.parse_args(bad)
    except argparse.ArgumentError as e:
        print("eoe err", bad, "->", str(e))
print("ArgumentError None:", str(argparse.ArgumentError(None, "bare error")))

# 21. Short-flag clustering edge cases & nargs=1 vs nargs=None
p_cl = argparse.ArgumentParser(prog="cl")
p_cl.add_argument("-a", action="store_true")
p_cl.add_argument("-b")
p_cl.add_argument("--single", nargs=1)
print("cluster val:", p_cl.parse_args(["-abhello"]))
print("cluster eq:", p_cl.parse_args(["-ab=world"]))
print("cluster unknown tail:", p_cl.parse_known_args(["-azzz"]))
print("nargs=1 list:", p_cl.parse_args(["--single", "only"]).single)

# 22. Non-list default copied via copy.copy in append/extend
class MyList(list):
    pass

p_cp = argparse.ArgumentParser(prog="cp")
orig_default = MyList([10])
p_cp.add_argument("--item", action="append", type=int, default=orig_default)
ns_cp = p_cp.parse_args(["--item", "20"])
print("copy append:", ns_cp.item, "orig untouched:", orig_default)

# 23. Nested subparsers + custom parser_class + custom metavar
class CustomParser(argparse.ArgumentParser):
    pass

p_top = argparse.ArgumentParser(prog="cli")
sp1 = p_top.add_subparsers(dest="grp", metavar="GROUP")
p_db = sp1.add_parser("db", help="database ops")
sp2 = p_db.add_subparsers(dest="op", parser_class=CustomParser)
p_mig = sp2.add_parser("migrate", help="run migrations")
p_mig.add_argument("--steps", type=int, default=1)
print("nested subparser:", p_top.parse_args(["db", "migrate", "--steps", "3"]))
print("custom parser class:", isinstance(p_mig, CustomParser))

# 24. sys.argv[1:] default when args=None, add_help=False, custom prefix_chars help
saved_argv = sys.argv
try:
    sys.argv = ["myscript.py", "--val", "99"]
    p_sys = argparse.ArgumentParser()
    p_sys.add_argument("--val", type=int)
    print("sys.argv parse:", p_sys.prog, p_sys.parse_args())
finally:
    sys.argv = saved_argv

p_nohelp = argparse.ArgumentParser(prog="nh", add_help=False)
p_nohelp.add_argument("--x")
print("nohelp usage:", repr(p_nohelp.format_usage()))

p_plushelp = argparse.ArgumentParser(prog="ph", prefix_chars="+")
print("plushelp usage:", repr(p_plushelp.format_usage()))

# 25. Long prog wrapping + mutex group usage (optional, required, suppressed)
saved_cols = os.environ.get("COLUMNS")
try:
    os.environ["COLUMNS"] = "40"
    p_lp = argparse.ArgumentParser(prog="a_very_very_very_very_long_program_name")
    m_opt = p_lp.add_mutually_exclusive_group(required=False)
    m_opt.add_argument("--alpha", action="store_true")
    m_opt.add_argument("--beta", action="store_true")
    m_opt.add_argument("--secret", action="store_true", help=argparse.SUPPRESS)
    m_req = p_lp.add_mutually_exclusive_group(required=True)
    m_req.add_argument("--gamma", action="store_true")
    m_req.add_argument("--delta", action="store_true")
    p_lp.add_argument("target")
    print("=== long prog wrapped usage ===")
    print(p_lp.format_usage())
finally:
    if saved_cols is None:
        if "COLUMNS" in os.environ:
            del os.environ["COLUMNS"]
    else:
        os.environ["COLUMNS"] = saved_cols

# 26. FileType invalid mode with "-"
try:
    argparse.FileType("q")("-")
except ValueError as e:
    print("FileType bad dash mode:", str(e))

# 27. parse_intermixed_args rejects positional in mutually exclusive group
p_mix_mx = argparse.ArgumentParser(prog="mixmx")
g_mx = p_mix_mx.add_mutually_exclusive_group()
g_mx.add_argument("pos", nargs="?")
try:
    p_mix_mx.parse_intermixed_args([])
except TypeError as e:
    print("intermixed mx TypeError:", str(e))

# 28. ArgumentDefaultsHelpFormatter preserves explicit %(default)s and skips SUPPRESS
p_ad = argparse.ArgumentParser(prog="ad", formatter_class=argparse.ArgumentDefaultsHelpFormatter)
p_ad.add_argument("--a", default=1, help="explicit %(default)s here")
p_ad.add_argument("--b", default=argparse.SUPPRESS, help="suppressed default")
p_ad.add_argument("--c", default=2, help="auto default")
print("=== ArgumentDefaultsHelpFormatter edge ===")
print(p_ad.format_help())

print("argparse ok")
