import contextlib
import io
import modulefinder
import os
import pyclbr
import tabnanny
import tempfile
import tokenize

with tempfile.TemporaryDirectory() as tmpdir:
    # -----------------------------------------------------------------------
    # 1. modulefinder
    # -----------------------------------------------------------------------
    pkg_dir = os.path.join(tmpdir, "mfpkg")
    os.makedirs(pkg_dir)
    with open(os.path.join(pkg_dir, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("PKG_CONST = 1\n")
    with open(os.path.join(pkg_dir, "helper.py"), "w", encoding="utf-8") as f:
        f.write("import math\nVAL = math.pi\n")
    script_path = os.path.join(tmpdir, "entry.py")
    with open(script_path, "w", encoding="utf-8") as f:
        f.write(
            "import mfpkg.helper\n"
            "import definitely_missing_mod_3249\n"
            "import excluded_mod_3249\n"
            "TOP_VAR = 99\n"
        )

    m = modulefinder.Module("demo", "demo.py", ["/tmp"])
    print("mf_module:", m.__name__, m.__file__, m.__path__, repr(m))

    finder = modulefinder.ModuleFinder(
        path=[tmpdir],
        excludes=["excluded_mod_3249"],
    )
    finder.run_script(script_path)
    missing, maybe = finder.any_missing_maybe()
    print(
        "mf_loaded:",
        "__main__" in finder.modules,
        "mfpkg" in finder.modules,
        "mfpkg.helper" in finder.modules,
        "math" in finder.modules,
        "TOP_VAR" in finder.modules["__main__"].globalnames,
    )
    print(
        "mf_missing:",
        finder.any_missing(),
        missing,
        maybe,
        "excluded_mod_3249" not in missing,
    )
    rep_buf = io.StringIO()
    with contextlib.redirect_stdout(rep_buf):
        finder.report()
    rep_out = rep_buf.getvalue()
    print(
        "mf_report:",
        "Name" in rep_out,
        "mfpkg.helper" in rep_out,
        "definitely_missing_mod_3249" in rep_out,
    )

    # -----------------------------------------------------------------------
    # 2. pyclbr
    # -----------------------------------------------------------------------
    browse_path = os.path.join(tmpdir, "sample_mod3249.py")
    with open(browse_path, "w", encoding="utf-8") as f:
        f.write(
            "class Base:\n"
            "    def base_m(self):\n"
            "        pass\n"
            "\n"
            "class Child(Base):\n"
            "    def method_a(self):\n"
            "        def inner_helper():\n"
            "            return 1\n"
            "        return inner_helper()\n"
            "    async def async_m(self):\n"
            "        return 2\n"
            "\n"
            "def top_func(x):\n"
            "    return x\n"
            "\n"
            "async def top_async():\n"
            "    return 3\n"
        )

    tree_ex = pyclbr.readmodule_ex("sample_mod3249", path=[tmpdir])
    tree_cls = pyclbr.readmodule("sample_mod3249", path=[tmpdir])
    print("pyclbr_keys:", sorted(tree_ex.keys()), sorted(tree_cls.keys()))
    base_cls = tree_ex["Base"]
    child_cls = tree_ex["Child"]
    top_fn = tree_ex["top_func"]
    top_afn = tree_ex["top_async"]
    print(
        "pyclbr_classes:",
        isinstance(base_cls, pyclbr.Class),
        base_cls.name,
        base_cls.lineno,
        base_cls.methods,
        isinstance(child_cls, pyclbr.Class),
        child_cls.lineno,
        child_cls.super == [base_cls],
        sorted(child_cls.methods.items()),
        "inner_helper" in child_cls.children["method_a"].children,
        child_cls.children["async_m"].is_async,
    )
    print(
        "pyclbr_funcs:",
        isinstance(top_fn, pyclbr.Function),
        top_fn.name,
        top_fn.lineno,
        top_fn.is_async,
        isinstance(top_afn, pyclbr.Function),
        top_afn.name,
        top_afn.lineno,
        top_afn.is_async,
    )

    # -----------------------------------------------------------------------
    # 3. tabnanny
    # -----------------------------------------------------------------------
    nag = tabnanny.NannyNag(5, "ambiguous indent", "\t x = 1\n")
    print(
        "nannynag:",
        issubclass(tabnanny.NannyNag, Exception),
        nag.get_lineno(),
        nag.get_msg(),
        repr(nag.get_line()),
    )

    # Clean code passes process_tokens
    clean_src = "def ok():\n    x = 1\n    if x:\n        return 2\n"
    tabnanny.process_tokens(tokenize.generate_tokens(io.StringIO(clean_src).readline))
    print("tabnanny_clean_tokens: ok")

    # Ambiguous indentation triggers NannyNag via process_tokens:
    # 1 tab vs 4 spaces (equal at tabsize=4, different at tabsize=1..3,5..8)
    ambig_src = "if True:\n    a = 1\n\tb = 2\n"
    try:
        tabnanny.process_tokens(tokenize.generate_tokens(io.StringIO(ambig_src).readline))
        print("tabnanny_ambig_tokens: missed")
    except tabnanny.NannyNag as exc:
        print("tabnanny_ambig_tokens:", exc.get_lineno(), "indent" in exc.get_msg())

    # tabnanny.check on clean vs ambiguous files
    clean_file = os.path.join(tmpdir, "clean.py")
    bad_file = os.path.join(tmpdir, "bad_tabs.py")
    with open(clean_file, "w", encoding="utf-8") as f:
        f.write(clean_src)
    with open(bad_file, "w", encoding="utf-8") as f:
        f.write("if True:\n\ta = 1\n        b = 2\n")

    out_clean = io.StringIO()
    err_clean = io.StringIO()
    with contextlib.redirect_stdout(out_clean), contextlib.redirect_stderr(err_clean):
        tabnanny.check(clean_file)
    print("tabnanny_check_clean:", out_clean.getvalue() == "", err_clean.getvalue() == "")

    out_bad = io.StringIO()
    err_bad = io.StringIO()
    with contextlib.redirect_stdout(out_bad), contextlib.redirect_stderr(err_bad):
        tabnanny.check(bad_file)
    combined_bad = out_bad.getvalue() + err_bad.getvalue()
    print("tabnanny_check_bad:", "bad_tabs.py" in combined_bad)
