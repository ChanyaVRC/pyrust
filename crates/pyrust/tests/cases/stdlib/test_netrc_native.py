# CPython 3.12.3 parity tests for the native `netrc` module (issue #3451).
#
# Everything printed here is compared byte-for-byte with CPython, so paths
# inside the temporary directory are shown relative to it.

import netrc
import os
import pathlib
import sys
import tempfile
from netrc import NetrcParseError, netrc as Netrc

# 1. Module surface -----------------------------------------------------------
print("public:", sorted(n for n in dir(netrc) if not n.startswith("__")))
print("__all__:", netrc.__all__)
print("doc:", netrc.__doc__)
print("class docs:", repr(NetrcParseError.__doc__), repr(Netrc.__doc__))
print("modules:", NetrcParseError.__module__, Netrc.__module__, netrc._netrclex.__module__)
print("os/stat attrs:", netrc.os is os, netrc.stat.__name__)
print("mro:", [c.__name__ for c in NetrcParseError.__mro__])

# 2. NetrcParseError ----------------------------------------------------------
e = NetrcParseError("boom", "file.netrc", 7)
print("err:", e.msg, e.filename, e.lineno, str(e), e.args, repr(e))
e = NetrcParseError("only msg")
print("err defaults:", e.msg, e.filename, e.lineno, str(e), e.args, repr(e))
e = NetrcParseError("kw", lineno=3, filename="x")
print("err kw:", str(e), e.args)
try:
    NetrcParseError()
except TypeError as exc:
    print("err no args:", exc)
try:
    NetrcParseError("a", "b", 1, 2)
except TypeError as exc:
    print("err too many:", exc)
try:
    raise NetrcParseError("raised", "f", 9)
except Exception as exc:
    print("is Exception:", isinstance(exc, Exception), type(exc).__name__, str(exc))


class Sub(NetrcParseError):
    pass


print("subclass:", str(Sub("m", "f", 1)), Sub("m", "f", 1).args)

# 3. Lexer --------------------------------------------------------------------
import io


def tokens(text):
    lex = netrc._netrclex(io.StringIO(text))
    out = []
    while True:
        tok = lex.get_token()
        out.append((tok, lex.lineno))
        if not tok:
            break
    return out


print("lex ws:", tokens("a b\tc\r\nd\n\n e"))
print("lex quotes:", tokens('"quoted tok" plain "" tail'))
print("lex escapes:", tokens('a\\ b "c\\"d" e\\\\f \\#g'))
print("lex escaped newline:", tokens("a\\\nb c"))
print("lex unterminated quote:", tokens('"abc def'))
print("lex trailing backslash:", tokens("abc\\"))
print("lex mid quote:", tokens('ab"cd ef'))
lex = netrc._netrclex(io.StringIO("x y"))
print("lex attrs:", lex.lineno, repr(lex.whitespace), lex.pushback, type(lex.instream).__name__)
lex.push_token("p1")
lex.push_token("p2")
print("lex pushback:", lex.get_token(), lex.get_token(), lex.get_token(), lex.get_token(), repr(lex.get_token()))
lex = netrc._netrclex(io.StringIO("a\nb"))
print("lex read_char:", repr(lex._read_char()), lex.lineno, repr(lex._read_char()), lex.lineno,
      repr(lex._read_char()), repr(lex._read_char()), lex.lineno)

with tempfile.TemporaryDirectory() as tmp:
    tmp = os.path.realpath(tmp)

    def write(name, content, mode="w"):
        path = os.path.join(tmp, name)
        if "b" in mode:
            with open(path, "wb") as fp:
                fp.write(content)
        else:
            with open(path, "w", encoding="utf-8", newline="") as fp:
                fp.write(content)
        return path

    def show(path):
        return path.replace(tmp, "<tmp>").replace(os.sep, "/") if isinstance(path, str) else repr(path)

    def parse(name, content):
        return Netrc(write(name, content))

    def error(name, content):
        path = write(name, content)
        try:
            Netrc(path)
        except NetrcParseError as exc:
            return ("NetrcParseError", exc.msg, show(exc.filename), exc.lineno,
                    str(exc).replace(path, show(path)), exc.args)
        except BaseException as exc:
            return (type(exc).__name__, str(exc))
        return ("OK",)

    # 4. Plain entries -------------------------------------------------------
    n = parse("plain.netrc",
              "machine example.com login alice password s3cr3t\n"
              "machine acct.example login bob account work password pw\n"
              "default login anon password guest\n")
    print("hosts:", n.hosts)
    print("macros:", n.macros)
    print("types:", type(n.hosts).__name__, type(n.macros).__name__,
          type(n.hosts["example.com"]).__name__)
    print("auth hit:", n.authenticators("example.com"))
    print("auth account:", n.authenticators("acct.example"))
    print("auth default fallback:", n.authenticators("nowhere.example"))
    print("repr:\n" + repr(n), end="")
    print("str same as repr:", str(n) == repr(n))

    n = parse("nodefault.netrc", "machine only.host login u password p\n")
    print("no default hit:", n.authenticators("only.host"))
    print("no default miss:", n.authenticators("other.host"))
    try:
        n.authenticators([])
    except TypeError as exc:
        print("auth unhashable:", exc)
    try:
        n.authenticators()
    except TypeError as exc:
        print("auth no args:", exc)

    # 5. user alias, defaults, ordering, duplicates ---------------------------
    n = parse("user.netrc", "machine h1 user uu password pp\nmachine h2 password only\nmachine h3\n")
    print("user alias:", n.hosts)
    print("repr user:\n" + repr(n), end="")
    n = parse("dup.netrc", "machine h login a password 1\nmachine h login b password 2\n")
    print("duplicate machine:", n.hosts, list(n.hosts))
    n = parse("kw_order.netrc", "machine h password pw account ac login lg\n")
    print("keyword order:", n.hosts)
    n = parse("repeat.netrc", "machine h login a login b password x password y\n")
    print("repeated keyword:", n.hosts)
    n = parse("empty_vals.netrc", 'machine h login "" password ""\n')
    print("empty values:", n.hosts)
    n = parse("empty.netrc", "")
    print("empty file:", n.hosts, n.macros, repr(repr(n)))
    n = parse("ws.netrc", "  \n\t\r\n")
    print("whitespace file:", n.hosts, n.macros)

    # 6. Quoting, escapes, special characters, comments -----------------------
    n = parse("special.netrc",
              "machine special.example\n"
              "  login alice\n"
              "  password !@#$%^&*()\n"
              "machine quoted.example\n"
              '  login "bob smith"\n'
              '  password "p@ss \\"word\\"\\\\end"\n'
              "machine esc.example login user\\ name password pass\\#word\n")
    print("special:", n.hosts)
    n = parse("comments.netrc",
              "# top comment\n"
              "machine c.example # trailing comment\n"
              "  login alice\n"
              "  # full line comment between keywords\n"
              "  password pw\n"
              "# another\n"
              "machine d.example\n"
              "login x\n"
              "password y # comment at end")
    print("comments:", n.hosts)
    n = parse("hash_tokens.netrc", "#word\nmachine h login #notcomment password p\n")
    print("hash tokens:", n.hosts)
    n = parse("hash_lone.netrc", "machine h login a # skipped rest of line\npassword b\n")
    print("lone hash:", n.hosts)
    n = parse("unicode.netrc", "machine hé.example login jürgen password pässö\n")
    print("unicode:", n.hosts)
    n = parse("crlf.netrc", "machine h login a\r\npassword b\r\n")
    print("crlf:", n.hosts)
    n = parse("cr.netrc", "machine h login a\rpassword b\r")
    print("cr only:", n.hosts)
    n = parse("noeol.netrc", "machine h login a password b")
    print("no trailing newline:", n.hosts)
    n = parse("multi.netrc", "machine a login 1\nmachine b login 2\ndefault login 3\nmachine c login 4\n")
    print("multi order:", list(n.hosts.items()))
    n = parse("tok_default.netrc", "default\n  login d\n  password dp\nmachine m login x password y\n")
    print("default first:", n.hosts, n.authenticators("zzz"))

    # 7. macdef ----------------------------------------------------------------
    n = parse("macro1.netrc", "macdef init\ncd /pub\nls -la\n\nmachine h login a password b\n")
    print("macro single:", n.macros, n.hosts)
    n = parse("macro2.netrc",
              "machine h login a password b\nmacdef one\nl1\nl2\n\nmacdef two\nonly\n\n"
              "machine i login c password d\n")
    print("macro multi:", n.macros, n.hosts)
    print("repr macros:\n" + repr(n), end="")
    n = parse("macro3.netrc", "macdef spaced name\nbody\n\n")
    print("macro spaced name:", n.macros)
    n = parse("macro4.netrc", "macdef x\n\nmachine h login a\n")
    print("macro immediate blank:", n.macros, n.hosts)
    n = parse("macro5.netrc", "macdef x\n  indented\n\ttabbed\n   \n\n")
    print("macro whitespace lines:", n.macros)
    n = parse("macro6.netrc", "macdef x\r\nbody\r\n\r\nmachine h login a\n")
    print("macro crlf:", n.macros, n.hosts)
    n = parse("macro7.netrc", "machine h login a macdef m\nbody\n\n")
    print("macro after entry:", n.hosts, n.macros)
    n = parse("macro8.netrc", "macdef dup\na\n\nmacdef dup\nb\n\n")
    print("macro redefined:", n.macros)
    n = parse("macro9.netrc", "macdef\nbody\n\n")
    print("macro missing name:", n.macros)
    n = parse("macro10.netrc", "macdef m # c\nbody\n\n")
    print("macro hash after name:", n.macros)
    n = parse("macro11.netrc", "macdef m\nlast line no newline\n\n# after\n")
    print("macro then comment:", n.macros)

    # 8. Errors -----------------------------------------------------------------
    print("bad toplevel:", error("e1.netrc", "bogus foo\n"))
    print("bad toplevel line 3:", error("e2.netrc", "# c\n\nbogus\n"))
    print("bad toplevel quote:", error("e2b.netrc", "'quoted' x\n"))
    print("bad toplevel empty-after:", error("e2c.netrc", "login alice\n"))
    print("missing machine name:", error("e3.netrc", "machine\n"))
    print("missing machine name quoted:", error("e3b.netrc", 'machine ""\n'))
    print("bad follower:", error("e4.netrc", "machine h\n  bogus val\n"))
    print("bad follower line 4:", error("e5.netrc", "machine h\nlogin a\n\npassword b\nbogus\n"))
    print("bad follower after entry:", error("e5b.netrc", "machine h login a Login b\n"))
    print("unterminated macdef:", error("e6.netrc", "macdef m\nline without blank terminator\n"))
    print("unterminated macdef empty body:", error("e7.netrc", "macdef m\n"))
    print("unterminated macdef no newline:", error("e8.netrc", "macdef m"))
    print("unterminated macdef after lines:", error("e9.netrc", "machine h login a\nmacdef m\na\nb"))
    print("macdef missing name at eof:", error("e10.netrc", "macdef"))
    print("login at eof:", error("e11.netrc", "machine h login"))
    print("password at eof:", error("e11b.netrc", "machine h login a password"))
    print("keyword as value:", error("e12.netrc", "machine h login machine\n"))
    print("keyword value swallow:", error("e13.netrc", "machine h login default password x\n"))
    print("escaped newline lineno:", error("e14.netrc", "machine h login a\\\nb\nbogus\n"))
    print("quoted newline lineno:", error("e15.netrc", 'machine h login "a\nb"\nbogus\n'))
    print("error partial hosts survive?:", end=" ")
    path = write("partial.netrc", "machine ok login a password b\nmachine h\n  bogus\n")
    try:
        Netrc(path)
    except NetrcParseError as exc:
        print(type(exc).__name__)

    # 9. file argument types -------------------------------------------------------
    path = write("argtypes.netrc", "machine h login a password b\n")
    print("str:", Netrc(path).hosts)
    print("keyword:", Netrc(file=path).hosts)
    print("Path:", Netrc(pathlib.Path(path)).hosts)
    print("bytes path:", Netrc(os.fsencode(path)).hosts)
    try:
        Netrc(pathlib.Path(write("pathbad.netrc", "bad\n")))
    except NetrcParseError as exc:
        print("Path error filename type:", type(exc.filename).__name__, show(str(exc.filename)), exc.lineno)

    class PathLike:
        def __fspath__(self):
            return path

    print("PathLike:", Netrc(PathLike()).hosts)
    try:
        Netrc(os.path.join(tmp, "missing.netrc"))
    except FileNotFoundError as exc:
        print("missing:", type(exc).__name__, exc.errno)
    try:
        Netrc(tmp)
    except OSError as exc:
        print("directory:", type(exc).__name__)
    try:
        Netrc(123456789)
    except OSError as exc:
        print("bad fd:", type(exc).__name__)
    try:
        Netrc(["not", "a", "path"])
    except TypeError as exc:
        print("bad type:", exc)
    try:
        Netrc(path, "extra")
    except TypeError as exc:
        print("too many args:", exc)
    try:
        Netrc.__init__()
    except TypeError as exc:
        print("init no self:", exc)

    # 10. encodings ---------------------------------------------------------------
    n = Netrc(write("utf8.netrc", "machine h login café password über\n".encode("utf-8"), "wb"))
    print("utf-8:", n.hosts)
    print("utf-8 bom:", ascii(error("bom.netrc", "﻿machine h login a password b\n")))
    n = Netrc(write("bom2.netrc", "machine ﻿h login a password b\n".encode("utf-8"), "wb"))
    print("embedded bom:", ascii(n.hosts))
    try:
        n = Netrc(write("latin.netrc", b"machine h login caf\xe9 password p\n", "wb"))
        print("locale fallback:", n.hosts)
    except BaseException as exc:
        print("locale fallback error:", type(exc).__name__)

    # 11. repr round trip ---------------------------------------------------------------
    src = ("machine alpha.example login alice account acct password secret\n"
           "default login anon password guest\n"
           "macdef init\ncd /pub\nls\n\n")
    n1 = parse("rt1.netrc", src)
    n2 = parse("rt2.netrc", repr(n1))
    print("round trip hosts:", n2.hosts == n1.hosts, "macros:", n2.macros == n1.macros)
    print("round trip repr identical:", repr(n2) == repr(n1))
    print("repr value:", repr(repr(n1)))

    # 12. Subclass hooks: _parse / _security_check / attribute-resolved lexer ---------
    calls = []

    class Tracing(Netrc):
        def _security_check(self, fp, default_netrc, login):
            calls.append((type(fp).__name__, default_netrc, login))
            return super()._security_check(fp, default_netrc, login)

    Tracing(write("trace.netrc", "machine a login x\nmachine b login anonymous\ndefault password z\n"))
    print("security check calls:", calls)
    calls.clear()
    Tracing(write("trace_empty.netrc", "# nothing\n"))
    print("security check calls (no entries):", calls)

    class Custom(Netrc):
        def _parse(self, file, fp, default_netrc):
            self.parsed = (show(file), type(fp).__name__, default_netrc)
            self.hosts["custom"] = ("l", "a", "p")

    c = Custom(path)
    print("custom _parse:", c.parsed, c.hosts, c.macros)
    print("instance dict:", sorted(vars(Netrc(path))))
    print("is instance:", isinstance(Netrc(path), Netrc), type(Netrc(path)).__name__)

    # 13. Default (~/.netrc) resolution -----------------------------------------------
    home = os.path.join(tmp, "home")
    os.mkdir(home)
    saved = {k: os.environ.get(k) for k in ("HOME", "USERPROFILE")}
    os.environ["HOME"] = home
    os.environ["USERPROFILE"] = home
    try:
        try:
            Netrc()
        except FileNotFoundError as exc:
            print("default missing:", type(exc).__name__, show(exc.filename))
        try:
            Netrc(None)
        except FileNotFoundError as exc:
            print("None missing:", type(exc).__name__)
        default_path = os.path.join(home, ".netrc")
        with open(default_path, "w", encoding="utf-8") as fp:
            fp.write("machine h login a password b\n")
        if os.name == "posix":
            os.chmod(default_path, 0o600)
        print("default ok:", Netrc().hosts)
        print("default ok None:", Netrc(None).hosts)
        if os.name == "posix":
            os.chmod(default_path, 0o644)
            try:
                Netrc()
            except NetrcParseError as exc:
                print("permissive:", exc.msg, exc.filename, exc.lineno, str(exc))
            with open(default_path, "w", encoding="utf-8") as fp:
                fp.write("machine h login anonymous password b\n")
            os.chmod(default_path, 0o644)
            print("anonymous exempt:", Netrc().hosts)
            with open(default_path, "w", encoding="utf-8") as fp:
                fp.write("# only a comment\n")
            os.chmod(default_path, 0o666)
            print("no entries, no check:", Netrc().hosts)
            # an explicit file is never permission-checked
            with open(default_path, "w", encoding="utf-8") as fp:
                fp.write("machine h login a password b\n")
            os.chmod(default_path, 0o666)
            print("explicit file not checked:", Netrc(default_path).hosts)
            os.chmod(default_path, 0o600)
    finally:
        for k, v in saved.items():
            if v is None:
                if k in os.environ:
                    del os.environ[k]
            else:
                os.environ[k] = v

print("done")
