# Issue #3266: `readline` stdlib module parity test.
#
# - On Windows (`sys.platform == "win32"`): verifies `import readline` raises
#   `ModuleNotFoundError`, matching CPython 3.12.3.
# - On Unix (`sys.platform != "win32"`): verifies history CRUD, 1-based
#   `get_history_item`, 0-based `remove_history_item` / `replace_history_item`,
#   history file read/write/append/truncation, completer & delimiter hooks,
#   `parse_and_bind`, `read_init_file`, and `get_line_buffer`.

import os
import sys

if sys.platform == "win32":
    try:
        import readline  # noqa: F401
        print("UNEXPECTED_IMPORT")
    except ModuleNotFoundError as exc:
        print("win32_missing_ok", type(exc).__name__)
else:
    import readline
    import tempfile

    # 1. Version constants
    print(
        "constants",
        isinstance(readline._READLINE_VERSION, int) and readline._READLINE_VERSION > 0,
        isinstance(readline._READLINE_RUNTIME_VERSION, int)
        and readline._READLINE_RUNTIME_VERSION > 0,
        isinstance(readline._READLINE_LIBRARY_VERSION, str)
        and len(readline._READLINE_LIBRARY_VERSION) > 0,
    )

    # 2. History CRUD (1-based get_history_item, 0-based remove/replace)
    readline.clear_history()
    print("empty_len", readline.get_current_history_length())

    readline.add_history("first")
    readline.add_history("second")
    readline.add_history("third")
    print(
        "after_add",
        readline.get_current_history_length(),
        readline.get_history_item(0),
        readline.get_history_item(-1),
        readline.get_history_item(1),
        readline.get_history_item(2),
        readline.get_history_item(3),
        readline.get_history_item(4),
    )

    readline.replace_history_item(1, "SECOND")
    print("after_replace", readline.get_history_item(2))

    readline.remove_history_item(0)
    print(
        "after_remove",
        readline.get_current_history_length(),
        readline.get_history_item(1),
        readline.get_history_item(2),
    )

    # Out-of-bounds errors
    err_kinds = []
    for fn, args in [
        (readline.remove_history_item, (-1,)),
        (readline.remove_history_item, (99,)),
        (readline.replace_history_item, (-1, "x")),
        (readline.replace_history_item, (99, "x")),
    ]:
        try:
            fn(*args)
            err_kinds.append("NO_ERROR")
        except ValueError:
            err_kinds.append("ValueError")
    print("history_errors", err_kinds)

    # 3. History file I/O & truncation
    orig_len = readline.get_history_length()
    print("default_history_length", orig_len)
    with tempfile.TemporaryDirectory() as tmpdir:
        hist_path = os.path.join(tmpdir, "pyrust_rl_hist.txt")
        readline.set_history_length(-1)
        readline.write_history_file(hist_path)
        with open(hist_path, "r", encoding="utf-8") as f:
            raw1 = f.read()
        print("written_raw", repr(raw1))

        readline.clear_history()
        readline.read_history_file(hist_path)
        print(
            "read_back",
            readline.get_current_history_length(),
            readline.get_history_item(1),
            readline.get_history_item(2),
        )

        readline.add_history("fourth")
        readline.add_history("fifth")
        readline.append_history_file(2, hist_path)
        with open(hist_path, "r", encoding="utf-8") as f:
            raw2 = f.read()
        print("appended_raw", repr(raw2))

        # Truncation via set_history_length
        readline.set_history_length(2)
        print("set_len_2", readline.get_history_length())
        readline.write_history_file(hist_path)
        with open(hist_path, "r", encoding="utf-8") as f:
            raw3 = f.read()
        print("truncated_raw", repr(raw3))
        readline.set_history_length(orig_len)

        # Negative nelements raises ValueError
        try:
            readline.append_history_file(-1, hist_path)
            print("neg_append", "NO_ERROR")
        except ValueError:
            print("neg_append", "ValueError")

        # Missing history file raises FileNotFoundError
        try:
            readline.read_history_file(os.path.join(tmpdir, "does_not_exist.hist"))
            print("missing_hist", "NO_ERROR")
        except FileNotFoundError:
            print("missing_hist", "FileNotFoundError")

        # read_init_file
        rc_path = os.path.join(tmpdir, "inputrc")
        with open(rc_path, "w", encoding="utf-8") as f:
            f.write("# comment\ntab: complete\n")
        readline.read_init_file(rc_path)
        print("read_init_ok", True)

        try:
            readline.read_init_file(os.path.join(tmpdir, "missing_inputrc"))
            print("missing_rc", "NO_ERROR")
        except FileNotFoundError:
            print("missing_rc", "FileNotFoundError")

    # 4. Auto-history, completer, delims, hooks, bindings, line buffer
    readline.set_auto_history(False)
    readline.set_auto_history(True)

    orig_completer = readline.get_completer()

    def sample_completer(text, state):
        return None

    readline.set_completer(sample_completer)
    print("completer_set", readline.get_completer() is sample_completer)
    readline.set_completer(None)
    print("completer_cleared", readline.get_completer() is None)

    try:
        readline.set_completer(12345)
        print("bad_completer", "NO_ERROR")
    except TypeError:
        print("bad_completer", "TypeError")

    orig_delims = readline.get_completer_delims()
    print(
        "delims_check",
        isinstance(orig_delims, str),
        " " in orig_delims,
        "\t" in orig_delims,
    )
    readline.set_completer_delims(" \t:")
    print("delims_updated", repr(readline.get_completer_delims()))
    readline.set_completer_delims(orig_delims)

    print(
        "indices",
        readline.get_completion_type(),
        readline.get_begidx(),
        readline.get_endidx(),
    )

    readline.set_startup_hook(lambda: None)
    readline.set_startup_hook(None)
    readline.set_pre_input_hook(lambda: None)
    readline.set_pre_input_hook(None)
    readline.set_completion_display_matches_hook(lambda *a: None)
    readline.set_completion_display_matches_hook(None)
    readline.parse_and_bind("tab: complete")
    print("line_buffer_type", isinstance(readline.get_line_buffer(), str))
    readline.redisplay()

    readline.set_completer(orig_completer)
    readline.clear_history()
    print("readline_parity_ok")
