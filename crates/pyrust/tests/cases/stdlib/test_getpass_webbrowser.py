import getpass
import io
import os
import sys
import warnings
import webbrowser

# ── 1. getpass ───────────────────────────────────────────────────────────────

assert issubclass(getpass.GetPassWarning, UserWarning)
assert getpass.GetPassWarning.__module__ == "getpass"

user = getpass.getuser()
assert isinstance(user, str) and len(user) > 0

# Environment variable precedence for getuser(): LOGNAME > USER > LNAME > USERNAME
saved_env = {k: os.environ.get(k) for k in ("LOGNAME", "USER", "LNAME", "USERNAME")}
try:
    for k in saved_env:
        if k in os.environ:
            del os.environ[k]
    os.environ["USERNAME"] = "u_username"
    assert getpass.getuser() == "u_username"
    os.environ["LNAME"] = "u_lname"
    assert getpass.getuser() == "u_lname"
    os.environ["USER"] = "u_user"
    assert getpass.getuser() == "u_user"
    os.environ["LOGNAME"] = "u_logname"
    assert getpass.getuser() == "u_logname"
finally:
    for k, v in saved_env.items():
        if v is None:
            if k in os.environ:
                del os.environ[k]
        else:
            os.environ[k] = v

# Non-tty fallback getpass() with monkeypatched sys.stdin and custom stream
orig_stdin = sys.stdin
try:
    sys.stdin = io.StringIO("s3cr3t-pass\n")
    out_buf = io.StringIO()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        pw = getpass.getpass("Enter secret: ", stream=out_buf)
    assert pw == "s3cr3t-pass"
    assert "Enter secret: " in out_buf.getvalue()
    assert any(issubclass(w.category, getpass.GetPassWarning) for w in caught)

    # EOF on empty stdin raises EOFError
    sys.stdin = io.StringIO("")
    out_buf2 = io.StringIO()
    with warnings.catch_warnings(record=True):
        warnings.simplefilter("always")
        try:
            getpass.getpass("Prompt: ", stream=out_buf2)
            assert False, "expected EOFError on empty stdin"
        except EOFError:
            pass
finally:
    sys.stdin = orig_stdin
print("getpass_ok")

# ── 2. webbrowser ────────────────────────────────────────────────────────────

assert issubclass(webbrowser.Error, Exception)
assert webbrowser.Error.__module__ == "webbrowser"
assert issubclass(webbrowser.GenericBrowser, webbrowser.BaseBrowser)
assert issubclass(webbrowser.BackgroundBrowser, webbrowser.GenericBrowser)

base = webbrowser.BaseBrowser("/usr/bin/sample-browser")
assert base.name == "/usr/bin/sample-browser"
assert base.basename == "/usr/bin/sample-browser"
try:
    base.open("https://example.test")
    assert False, "expected NotImplementedError"
except NotImplementedError:
    pass

gen = webbrowser.GenericBrowser(["/usr/bin/fake-browser", "--open", "%s"])
assert gen.name == "/usr/bin/fake-browser"
assert gen.basename == "fake-browser"
assert gen.args == ["--open", "%s"]

gen_str = webbrowser.GenericBrowser("simple-browser")
assert gen_str.name == "simple-browser"
assert gen_str.basename == "simple-browser"
assert gen_str.args == ["%s"]

# Register custom mock browser (never launches a real browser!)
calls = []


class MockBrowser(webbrowser.BaseBrowser):
    def __init__(self, name="mock"):
        super().__init__(name)

    def open(self, url, new=0, autoraise=True):
        calls.append((url, new, autoraise))
        return True


class FactoryBrowser(webbrowser.BaseBrowser):
    def __init__(self):
        super().__init__("factory-mock")

    def open(self, url, new=0, autoraise=True):
        calls.append(("factory", url, new, autoraise))
        return True


mock_inst = MockBrowser("mock-one")
webbrowser.register("mock-browser-3253", MockBrowser, mock_inst, preferred=True)
webbrowser.register("factory-browser-3253", FactoryBrowser)

try:
    assert webbrowser.get("mock-browser-3253") is mock_inst
    assert webbrowser.get("MOCK-BROWSER-3253") is mock_inst
    assert webbrowser.get() is mock_inst

    fb = webbrowser.get("factory-browser-3253")
    assert isinstance(fb, FactoryBrowser)
    assert fb.name == "factory-mock"

    assert webbrowser.open("https://example.test/one") is True
    assert webbrowser.open_new("https://example.test/two") is True
    assert webbrowser.open_new_tab("https://example.test/three") is True
    assert mock_inst.open_new("https://example.test/four") is True
    assert mock_inst.open_new_tab("https://example.test/five") is True

    assert calls == [
        ("https://example.test/one", 0, True),
        ("https://example.test/two", 1, True),
        ("https://example.test/three", 2, True),
        ("https://example.test/four", 1, True),
        ("https://example.test/five", 2, True),
    ]

    try:
        webbrowser.get("nonexistent-browser-xyz-3253")
        assert False, "expected webbrowser.Error"
    except webbrowser.Error:
        pass
finally:
    # Clean up our test registrations from the module registry
    for key in ("mock-browser-3253", "factory-browser-3253"):
        if webbrowser._browsers and key in webbrowser._browsers:
            del webbrowser._browsers[key]
        if webbrowser._tryorder:
            while key in webbrowser._tryorder:
                webbrowser._tryorder.remove(key)

print("webbrowser_ok")
