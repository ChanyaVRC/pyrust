# Parity fixture for `site`, `_sitebuiltins`, interactive builtins, and
# `sys.flags` structseq metadata (issue #3243).

import builtins
import contextlib
import io
import os
import site
import sys
import tempfile
import _sitebuiltins


# ── 1. Startup `site` & interactive builtins ─────────────────────────────────

print("site in sys.modules:", "site" in sys.modules)
print("_sitebuiltins in sys.modules:", "_sitebuiltins" in sys.modules)
for name in ("quit", "exit", "copyright", "credits", "license", "help"):
    print(f"builtin {name}:", hasattr(builtins, name), callable(getattr(builtins, name)))

print("isinstance quit Quitter:", isinstance(builtins.quit, _sitebuiltins.Quitter))
print("isinstance exit Quitter:", isinstance(builtins.exit, _sitebuiltins.Quitter))
print("isinstance copyright _Printer:", isinstance(builtins.copyright, _sitebuiltins._Printer))
print("isinstance credits _Printer:", isinstance(builtins.credits, _sitebuiltins._Printer))
print("isinstance license _Printer:", isinstance(builtins.license, _sitebuiltins._Printer))
print("isinstance help _Helper:", isinstance(builtins.help, _sitebuiltins._Helper))


# ── 2. `_sitebuiltins.Quitter`, `_Printer`, `_Helper` ─────────────────────────

q = _sitebuiltins.Quitter("leave", "Ctrl-X")
print("quitter attrs:", q.name, q.eof, repr(q))
for call_arg in (None, 0, 7, "bye"):
    try:
        if call_arg is None:
            q()
        else:
            q(call_arg)
    except SystemExit as exc:
        print("quitter exit:", repr(call_arg), repr(exc.code), exc.args)

p_short = _sitebuiltins._Printer("short", "line1\nline2\nline3")
print("printer MAXLINES:", _sitebuiltins._Printer.MAXLINES, p_short.MAXLINES)
print("printer short repr:", repr(p_short))
buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    ret = p_short()
print("printer short call:", repr(ret), repr(buf.getvalue()))

many_lines = "\n".join(f"L{i}" for i in range(30))
p_long = _sitebuiltins._Printer("longtext", many_lines)
print("printer long repr:", repr(p_long))

# Paged call with mocked input('q') to stop after first page.
orig_input = builtins.input
prompts = []
try:
    def fake_input(prompt=""):
        prompts.append(prompt)
        return "q"
    builtins.input = fake_input
    buf2 = io.StringIO()
    with contextlib.redirect_stdout(buf2):
        p_long()
    out_lines = buf2.getvalue().splitlines()
    print("printer paged count:", len(out_lines), out_lines[0], out_lines[-1], len(prompts))
finally:
    builtins.input = orig_input

# _Printer loading text from file on disk.
with tempfile.TemporaryDirectory() as td:
    with open(os.path.join(td, "NOTICE.txt"), "w", encoding="utf-8") as f:
        f.write("Loaded from NOTICE file")
    p_file = _sitebuiltins._Printer("notice", "fallback", ["MISSING.txt", "NOTICE.txt"], [td])
    print("printer file repr:", repr(p_file))
    p_fallback = _sitebuiltins._Printer("notice", "fallback text", ["NOPE.txt"], [td])
    print("printer fallback repr:", repr(p_fallback))

h = _sitebuiltins._Helper()
print("helper repr:", repr(h))


# ── 3. `site` metadata, prefixes, userbase, usersite, getsitepackages ────────

print("PREFIXES type/len:", isinstance(site.PREFIXES, list), len(site.PREFIXES) >= 2)
print("ENABLE_USER_SITE type:", site.ENABLE_USER_SITE in (True, False, None))
print("check_enableusersite:", site.check_enableusersite() in (True, False, None))
print("getuserbase str:", isinstance(site.getuserbase(), str), site.USER_BASE == site.getuserbase())
print(
    "getusersitepackages str:",
    isinstance(site.getusersitepackages(), str),
    site.USER_SITE == site.getusersitepackages(),
)
print("usersite contains site-packages:", "site-packages" in site.getusersitepackages())

custom_prefix = os.path.join(os.sep, "opt", "custom_py")
sp = site.getsitepackages([custom_prefix])
print("getsitepackages custom count:", len(sp))
print("getsitepackages custom last ends with site-packages:", sp[-1].endswith("site-packages"))
print("getsitepackages custom dedup:", site.getsitepackages([custom_prefix, custom_prefix]) == sp)
print("getsitepackages empty:", site.getsitepackages([""]))


# ── 4. `makepath`, `removeduppaths`, `addsitedir`, `addpackage` (.pth) ───────

mp_dir, mp_case = site.makepath("a", "b")
print("makepath isabs:", os.path.isabs(mp_dir), mp_case == os.path.normcase(mp_dir))

saved_path = list(sys.path)
try:
    with tempfile.TemporaryDirectory() as td:
        sub_alpha = os.path.join(td, "alpha_pkg")
        sub_beta = os.path.join(td, "beta_pkg")
        os.makedirs(sub_alpha)
        os.makedirs(sub_beta)
        with open(os.path.join(sub_alpha, "mod_alpha_3243.py"), "w", encoding="utf-8") as f:
            f.write("TAG = 'from_alpha'\n")
        with open(os.path.join(sub_beta, "mod_beta_3243.py"), "w", encoding="utf-8") as f:
            f.write("TAG = 'from_beta'\n")

        # a.pth adds alpha_pkg and executes an import statement; b.pth adds beta_pkg
        # and repeats alpha_pkg plus a missing dir.
        with open(os.path.join(td, "a.pth"), "w", encoding="utf-8") as f:
            f.write("# comment line\n\nalpha_pkg\nimport sys; sys._pth_probe_3243 = 'ran'\n")
        with open(os.path.join(td, "b.pth"), "w", encoding="utf-8") as f:
            f.write("alpha_pkg\nbeta_pkg\ndoes_not_exist_dir\n")
        with open(os.path.join(td, ".hidden.pth"), "w", encoding="utf-8") as f:
            f.write("import sys; sys._pth_hidden_3243 = 'bad'\n")

        known = set()
        ret_known = site.addsitedir(td, known)
        print("addsitedir returns known_paths:", ret_known is known)
        print("pth import ran:", getattr(sys, "_pth_probe_3243", None))
        print("hidden pth skipped:", hasattr(sys, "_pth_hidden_3243"))
        delattr(sys, "_pth_probe_3243")

        import mod_alpha_3243
        import mod_beta_3243
        print("imported via pth:", mod_alpha_3243.TAG, mod_beta_3243.TAG)

        # Verify ordering: td, alpha_pkg, beta_pkg, no duplicate alpha_pkg, no missing dir.
        abs_td = os.path.abspath(td)
        abs_alpha = os.path.abspath(sub_alpha)
        abs_beta = os.path.abspath(sub_beta)
        print(
            "pth order & dedup:",
            sys.path.count(abs_td),
            sys.path.count(abs_alpha),
            sys.path.count(abs_beta),
            sys.path.index(abs_td) < sys.path.index(abs_alpha) < sys.path.index(abs_beta),
            any("does_not_exist_dir" in p for p in sys.path),
        )

        # known_paths=None returns None.
        print("addsitedir None return:", site.addsitedir(td, None) is None)
        print("addpackage None return:", site.addpackage(td, "b.pth", None) is None)

        # removeduppaths deduplicates sys.path in place.
        sys.path = [abs_alpha, abs_beta, abs_alpha]
        dedup_known = site.removeduppaths()
        print("removeduppaths:", len(sys.path), isinstance(dedup_known, set), len(dedup_known))
finally:
    sys.path[:] = saved_path
    sys.modules.pop("mod_alpha_3243", None)
    sys.modules.pop("mod_beta_3243", None)


# ── 5. `sys.flags` 18-field structseq parity ─────────────────────────────────

flag_fields = (
    "debug",
    "inspect",
    "interactive",
    "optimize",
    "dont_write_bytecode",
    "no_user_site",
    "no_site",
    "ignore_environment",
    "verbose",
    "bytes_warning",
    "quiet",
    "hash_randomization",
    "isolated",
    "dev_mode",
    "utf8_mode",
    "warn_default_encoding",
    "safe_path",
    "int_max_str_digits",
)
print(
    "sys.flags shape:",
    isinstance(sys.flags, tuple),
    len(sys.flags),
    sys.flags.n_fields,
    sys.flags.n_sequence_fields,
    sys.flags.n_unnamed_fields,
)
print("sys.flags all attrs:", all(hasattr(sys.flags, f) for f in flag_fields))
print(
    "sys.flags types:",
    type(sys.flags.dev_mode) is bool,
    type(sys.flags.safe_path) is bool,
    type(sys.flags.optimize) is int,
    type(sys.flags.int_max_str_digits) is int,
)
print("sys.flags index matches attr:", sys.flags[3] == sys.flags.optimize, sys.flags[17] == sys.flags.int_max_str_digits)
