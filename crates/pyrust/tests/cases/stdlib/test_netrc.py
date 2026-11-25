# Comprehensive CPython 3.12 parity tests for `netrc` (issue #3230).

import netrc
import os
import sys
import tempfile


def write_and_parse(tmp, name, content):
    path = os.path.join(tmp, name)
    with open(path, "w", encoding="utf-8", newline="\n") as fp:
        fp.write(content)
    return path, netrc.netrc(path)


def expect_parse_error(tmp, name, content):
    path = os.path.join(tmp, name)
    with open(path, "w", encoding="utf-8", newline="\n") as fp:
        fp.write(content)
    try:
        netrc.netrc(path)
        return ("OK",)
    except netrc.NetrcParseError as e:
        return (
            type(e).__name__,
            e.msg,
            os.path.basename(e.filename) if e.filename else None,
            e.lineno,
            str(e).replace(path, os.path.basename(path)),
        )


# 1. Module metadata and exception
print("in sys.modules:", "netrc" in sys.modules)
print("__all__:", netrc.__all__)
print("NetrcParseError.__module__:", netrc.NetrcParseError.__module__)
print("netrc.__module__:", netrc.netrc.__module__)
err = netrc.NetrcParseError("boom", "sample.netrc", 7)
print("err attrs:", err.msg, err.filename, err.lineno, str(err))

with tempfile.TemporaryDirectory() as tmp:
    # 2. Comprehensive valid .netrc with comments, quotes, escapes, user/login, account, password, default, macdef
    sample = (
        "# Top-level comment\n"
        "machine example.com\n"
        "  login alice\n"
        "  account acct1\n"
        "  password s3cr3t#pass\n"
        "\n"
        "machine quoted.example # inline comment\n"
        '  user "bob smith"\n'
        '  password "p@ss \\"word\\"\\\\end"\n'
        "\n"
        "machine escaped.host\n"
        "  login user\\ name\n"
        "  password pass\\#word\n"
        "\n"
        "default login anon password guest\n"
        "\n"
        "macdef init\n"
        "cd /pub\n"
        "ls -la\n"
        "\n"
    )
    _, n = write_and_parse(tmp, "valid.netrc", sample)
    print("hosts:", n.hosts)
    print("macros:", n.macros)
    print("auth example.com:", n.authenticators("example.com"))
    print("auth quoted.example:", n.authenticators("quoted.example"))
    print("auth escaped.host:", n.authenticators("escaped.host"))
    print("auth unknown (uses default):", n.authenticators("unknown.host"))
    print("repr:\n" + repr(n))

    # 3. No default entry -> authenticators returns None for unknown host
    _, n_nodef = write_and_parse(
        tmp,
        "nodef.netrc",
        "machine only.host login u password p\n",
    )
    print("nodef known:", n_nodef.authenticators("only.host"))
    print("nodef unknown:", n_nodef.authenticators("other.host"))

    # 4. Empty file
    _, n_empty = write_and_parse(tmp, "empty.netrc", "# only comments\n\n")
    print("empty:", n_empty.hosts, n_empty.macros, repr(n_empty))

    # 5. Round-trip repr() back through netrc() (for whitespace-free tokens)
    simple_src = (
        "machine alpha.example login alice account acct password secret\n"
        "default login anon password guest\n"
        "macdef init\n"
        "cd /pub\n"
        "ls\n"
        "\n"
    )
    _, n_simple = write_and_parse(tmp, "simple.netrc", simple_src)
    _, n_rt = write_and_parse(tmp, "roundtrip.netrc", repr(n_simple))
    print("roundtrip hosts eq:", n_rt.hosts == n_simple.hosts)
    print("roundtrip macros eq:", n_rt.macros == n_simple.macros)

    # 6. Error cases matching CPython 3.12 messages and line numbers
    print("bad toplevel:", expect_parse_error(tmp, "bad_top.netrc", "bogus foo\n"))
    print("missing machine name:", expect_parse_error(tmp, "no_name.netrc", "machine\n"))
    print("bad follower:", expect_parse_error(tmp, "bad_fol.netrc", "machine h\n  bogus val\n"))
    print(
        "unterminated macdef:",
        expect_parse_error(tmp, "bad_mac.netrc", "macdef m\nline without blank terminator\n"),
    )

    # 7. Missing file raises FileNotFoundError
    try:
        netrc.netrc(os.path.join(tmp, "does_not_exist.netrc"))
        print("missing file: OK")
    except FileNotFoundError:
        print("missing file: FileNotFoundError")
