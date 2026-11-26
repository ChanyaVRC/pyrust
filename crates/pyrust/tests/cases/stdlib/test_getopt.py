# Comprehensive CPython 3.12 parity tests for `getopt` (issue #3230).

import getopt
import os
import sys


def expect_exc(fn, *args, **kwargs):
    try:
        return ("OK", fn(*args, **kwargs))
    except getopt.GetoptError as e:
        return (type(e).__name__, e.msg, e.opt, str(e))
    except Exception as e:
        return (type(e).__name__, str(e))


# 1. Module metadata and exception hierarchy
print("in sys.modules:", "getopt" in sys.modules)
print("__all__:", getopt.__all__)
print("error is GetoptError:", getopt.error is getopt.GetoptError)
print("GetoptError.__module__:", getopt.GetoptError.__module__)
err = getopt.GetoptError("bad option", "x")
print("err attrs:", err.msg, err.opt, str(err), err.args)

# 2. Standard getopt: short options, attached args, bundled flags, long options, '--', '-'
print("short basic:", getopt.getopt(["-a", "-b", "val", "pos1", "-c"], "ab:c"))
print("short bundled:", getopt.getopt(["-ac", "-bval", "rest"], "ab:c"))
print("short bundled with arg:", getopt.getopt(["-abval", "rest"], "ab:"))
print(
    "long basic:",
    getopt.getopt(
        ["--alpha", "--beta=123", "--gamma", "456", "pos"],
        "",
        ["alpha", "beta=", "gamma="],
    ),
)
print("long str:", getopt.getopt(["--only=yes", "arg"], "", "only="))
print("long prefix unique:", getopt.getopt(["--al", "--be=1"], "", ["alpha", "beta="]))
print("long exact over prefix:", getopt.getopt(["--foo"], "", ["foo", "foobar"]))
print("long exact with arg:", getopt.getopt(["--foo=1"], "", ["foo=", "foobar="]))
print("double dash:", getopt.getopt(["-a", "--", "-b", "--alpha"], "ab", ["alpha"]))
print("single dash:", getopt.getopt(["-a", "-", "-b"], "ab"))
print("empty args:", getopt.getopt([], "ab:", ["alpha="]))
print("repeated opts:", getopt.getopt(["-v", "-v", "--item=1", "--item=2"], "v", ["item="]))

# 3. getopt errors (exact message and .opt attribute)
print("err unknown short:", expect_exc(getopt.getopt, ["-z"], "ab:"))
print("err missing short arg:", expect_exc(getopt.getopt, ["-b"], "ab:"))
print("err colon as opt:", expect_exc(getopt.getopt, ["-:"], "a:b"))
print("err unknown long:", expect_exc(getopt.getopt, ["--nope"], "", ["alpha"]))
print("err missing long arg:", expect_exc(getopt.getopt, ["--beta"], "", ["beta="]))
print("err unexpected long arg:", expect_exc(getopt.getopt, ["--alpha=1"], "", ["alpha"]))
print("err ambiguous prefix:", expect_exc(getopt.getopt, ["--al"], "", ["alpha", "always"]))

# 4. gnu_getopt: intermixed options and positional args
old_posix = os.environ.get("POSIXLY_CORRECT")
if old_posix is not None:
    del os.environ["POSIXLY_CORRECT"]
try:
    print(
        "gnu intermixed:",
        getopt.gnu_getopt(
            ["pos1", "-a", "pos2", "--beta=42", "-b", "v", "pos3"],
            "ab:",
            ["beta="],
        ),
    )
    print(
        "gnu double dash:",
        getopt.gnu_getopt(["pos1", "-a", "--", "-b", "pos2"], "ab"),
    )
    print(
        "gnu single dash:",
        getopt.gnu_getopt(["-a", "-", "-b"], "ab"),
    )
    print(
        "gnu plus stops:",
        getopt.gnu_getopt(["-a", "pos1", "-b"], "+ab"),
    )

    os.environ["POSIXLY_CORRECT"] = "1"
    print(
        "gnu POSIXLY_CORRECT:",
        getopt.gnu_getopt(["-a", "pos1", "-b"], "ab"),
    )
finally:
    if old_posix is None:
        if "POSIXLY_CORRECT" in os.environ:
            del os.environ["POSIXLY_CORRECT"]
    else:
        os.environ["POSIXLY_CORRECT"] = old_posix

# 5. gnu_getopt errors
print("gnu err unknown:", expect_exc(getopt.gnu_getopt, ["pos", "--bad"], "", ["good"]))
print("gnu err missing arg:", expect_exc(getopt.gnu_getopt, ["pos", "-x"], "x:"))

# 6. Exception object details (issue #3330)
e = getopt.GetoptError("m")
print("err default opt:", repr(e.opt), e.msg, e.args, repr(e), str(e))
e = getopt.GetoptError("m", "o")
print("err repr:", repr(e), e.args)
print("err class attrs:", repr(getopt.GetoptError.opt), repr(getopt.GetoptError.msg))
print("err mro:", [c.__name__ for c in getopt.GetoptError.__mro__])
print("err is Exception:", issubclass(getopt.GetoptError, Exception), getopt.error.__name__)
print("err missing msg:", expect_exc(getopt.GetoptError)[:1])
print("err kw:", expect_exc(lambda: getopt.GetoptError(msg="a", opt="b").args))
print("err too many:", expect_exc(getopt.GetoptError, 1, 2, 3)[:1])
print("err str nonstr:", str(getopt.GetoptError("msg", None)), getopt.GetoptError("msg", None).opt)


class MyErr(getopt.GetoptError):
    pass


print("err subclass:", repr(MyErr("x", "y")), str(MyErr("x", "y")), MyErr("x").opt == "")
try:
    raise getopt.error("boom", "b")
except getopt.GetoptError as exc:
    print("raise alias:", exc.msg, exc.opt, exc.args, type(exc).__name__)
import pickle

print("err pickle:", pickle.loads(pickle.dumps(getopt.GetoptError("pm", "po"))).args)

# 7. Module surface
print("module public names:", sorted(n for n in dir(getopt) if not n.startswith("__")))
print("module has os:", getopt.os is os, getopt._("x"))
print("doc head:", getopt.__doc__.splitlines()[0], len(getopt.__doc__.splitlines()))
print("name:", getopt.__name__)
print("fn names:", getopt.getopt.__name__, getopt.gnu_getopt.__name__, getopt.do_longs.__name__)

# 8. Argument container types and keyword arguments
print("tuple args:", getopt.getopt(("-a", "x", "y"), "a"))
print("tuple args no opts:", getopt.getopt(("x", "-a"), "a"))
print("list identity:", (lambda a: getopt.getopt(a, "a")[1] is a)(["x"]))
print("kwargs:", getopt.getopt(args=["-a", "z"], shortopts="a", longopts=[]))
print("gnu kwargs:", getopt.gnu_getopt(args=["z", "-a"], shortopts="a"))
print("longopts tuple:", getopt.getopt(["--aa", "--bb=1"], "", ("aa", "bb=")))
print("longopts generator:", getopt.getopt(["--aa"], "", (x for x in ["aa"])))
print("falsy args:", getopt.getopt(None, "a"), getopt.getopt((), "a"), getopt.getopt("", "a"))
print("str args:", getopt.getopt("-a", "a"), getopt.getopt("ab", "a"))
print("nonstr arg:", expect_exc(getopt.getopt, [1, 2], "a"))
print("nonstr gnu arg:", expect_exc(getopt.gnu_getopt, [1, 2], "a"))
print("gnu list elem:", getopt.gnu_getopt([["x"], "-a"], "a"))
print("bad args type:", expect_exc(getopt.getopt, 5, "a"))
print("bad longopts:", expect_exc(getopt.getopt, ["-a"], "a", 5))
print("bad longopts elem:", expect_exc(getopt.getopt, ["--a"], "", [1]))
print("bad shortopts:", expect_exc(getopt.getopt, ["-a"], None))
print("bad shortopts gnu:", expect_exc(getopt.gnu_getopt, ["-a"], None))
print("no args given:", expect_exc(getopt.getopt)[:1])
print("one arg given:", expect_exc(getopt.getopt, [])[:1])

# 9. Edge cases in short and long parsing
print("empty shortopts:", expect_exc(getopt.getopt, ["-a"], ""))
print("bare colon opts:", expect_exc(getopt.getopt, ["-a", "-:"], "a:"))
print("plus in getopt:", expect_exc(getopt.getopt, ["-+", "-a"], "+a"))
print("attached empty:", getopt.getopt(["-a="], "a:"))
print("short arg next:", getopt.getopt(["-a", "-b", "x"], "a:b"))
print("short arg dashdash:", getopt.getopt(["-a", "--", "x"], "a:"))
print("long arg next dash:", getopt.getopt(["--aa", "-x", "r"], "x", ["aa="]))
print("long empty value:", getopt.getopt(["--aa="], "", ["aa="]))
print("long equals in value:", getopt.getopt(["--aa=b=c"], "", ["aa="]))
print("long prefix with arg:", getopt.getopt(["--al=5"], "", ["alpha="]))
print("long prefix unique no arg:", getopt.getopt(["--al"], "", ["alpha"]))
print("long exact beats longer:", getopt.getopt(["--a"], "", ["a", "ab", "ac"]))
print("long a= beats longer:", getopt.getopt(["--a=1"], "", ["a=", "ab"]))
print("long ambiguous both arg:", expect_exc(getopt.getopt, ["--a=1"], "", ["ab=", "ac="]))
print("long empty name:", expect_exc(getopt.getopt, ["--=x"], "", ["a"]))
print("long empty name any:", expect_exc(getopt.getopt, ["--="], "", ["a", "b"]))
print("long equals only prefix:", expect_exc(getopt.getopt, ["--a=b"], "", ["a=b"]))
print("long unicode:", getopt.getopt(["--é=ü", "-é"], "é", ["é="]))
print("long needs arg last:", expect_exc(getopt.getopt, ["x", "--aa"], "", ["aa="]))
print("long needs arg only:", expect_exc(getopt.getopt, ["--aa"], "", ["aa="]))
print("long no arg given:", expect_exc(getopt.getopt, ["--aa=1"], "", ["aa"]))
print("long no arg prefix:", expect_exc(getopt.getopt, ["--a=1"], "", ["aa"]))
print("long arg value empty:", getopt.getopt(["--aa", ""], "", ["aa="]))
print("long single str:", getopt.getopt(["--aa"], "", "aa"))
print("short unicode:", getopt.getopt(["-éx"], "éx"))
print("short missing second:", expect_exc(getopt.getopt, ["-ab"], "ab:"))
print("short partial then error:", expect_exc(getopt.getopt, ["-az"], "a"))
print("short nonstr optarg:", getopt.getopt(["-a", 5], "a:"))
print("long nonstr optarg:", getopt.getopt(["--aa", 5], "", ["aa="]))
print("remaining after opts:", getopt.getopt(["-a", "1", "-b"], "ab"))

# 10. gnu_getopt details
def unset_posix():
    if "POSIXLY_CORRECT" in os.environ:
        del os.environ["POSIXLY_CORRECT"]


unset_posix()
print("gnu empty:", getopt.gnu_getopt([], "a"))
print("gnu only pos:", getopt.gnu_getopt(["a", "b"], "a"))
print("gnu tuple in:", getopt.gnu_getopt(("x", "-a"), "a"))
print("gnu leading dashdash:", getopt.gnu_getopt(["--", "-a"], "a"))
print("gnu +empty:", getopt.gnu_getopt(["x", "-a"], "+"))
print("gnu +long:", getopt.gnu_getopt(["--aa", "x", "--aa"], "+", ["aa"]))
print("gnu + not first:", expect_exc(getopt.gnu_getopt, ["-+"], "a+"))
print("gnu + not stripped twice:", expect_exc(getopt.gnu_getopt, ["-+"], "++"))
print("gnu longs:", getopt.gnu_getopt(["x", "--al=1", "y", "--be", "z", "w"], "", ["alpha=", "beta="]))
print("gnu missing long arg:", expect_exc(getopt.gnu_getopt, ["x", "--be"], "", ["beta="]))
print("gnu short arg is dashdash:", getopt.gnu_getopt(["-a", "--", "x"], "a:"))
print("gnu short attached:", getopt.gnu_getopt(["x", "-a1", "y"], "a:"))
for value in ("1", "yes", "0"):
    os.environ["POSIXLY_CORRECT"] = value
    print("gnu POSIXLY_CORRECT", value, getopt.gnu_getopt(["a", "-x", "b"], "x"))
os.environ["POSIXLY_CORRECT"] = ""
print("gnu POSIXLY_CORRECT empty:", getopt.gnu_getopt(["a", "-x", "b"], "x"))
os.environ["POSIXLY_CORRECT"] = "1"
print("gnu posix plus ok:", getopt.gnu_getopt(["a", "-x"], "+x"))
print("gnu posix dashdash:", getopt.gnu_getopt(["-x", "--", "-x"], "x"))
unset_posix()
print("gnu after unset:", getopt.gnu_getopt(["a", "-x", "b"], "x"))

# 11. Public helper functions
acc = []
print("do_longs:", getopt.do_longs(acc, "al=3", ["alpha="], ["r"]), acc)
acc = []
print("do_longs next:", getopt.do_longs(acc, "al", ["alpha="], ["v", "r"]), acc)
acc = []
print("do_longs err:", expect_exc(getopt.do_longs, acc, "zz", ["alpha="], []), acc)
acc = []
print("do_shorts:", getopt.do_shorts(acc, "ab1", "ab:", ["r"]), acc)
acc = []
print("do_shorts next:", getopt.do_shorts(acc, "ab", "ab:", ["v", "r"]), acc)
acc = []
print("do_shorts partial err:", expect_exc(getopt.do_shorts, acc, "az", "a", []), acc)
print("long_has_args:", getopt.long_has_args("al", ["alpha=", "beta"]), getopt.long_has_args("be", ["alpha=", "beta"]))
print("long_has_args exact:", getopt.long_has_args("a", ["a", "ab"]), getopt.long_has_args("a", ["a=", "ab"]))
print("long_has_args err:", expect_exc(getopt.long_has_args, "a", ["ab", "ac"]), expect_exc(getopt.long_has_args, "q", ["ab"]))
print("short_has_arg:", getopt.short_has_arg("a", "a:b"), getopt.short_has_arg("b", "a:b"))
print("short_has_arg err:", expect_exc(getopt.short_has_arg, ":", "a:"), expect_exc(getopt.short_has_arg, "ab", "ab"), expect_exc(getopt.short_has_arg, "", "ab"))
