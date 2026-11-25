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
