# Comprehensive cross-platform CPython 3.12 parity tests for `subprocess`.

import os
import pathlib
import subprocess
import sys
import tempfile

# --- Constants, __all__, and exception hierarchy ---
print("constants", subprocess.PIPE, subprocess.STDOUT, subprocess.DEVNULL)
print("all-exact", subprocess.__all__ == sorted(subprocess.__all__) or isinstance(subprocess.__all__, list))
for name in (
    "Popen",
    "PIPE",
    "STDOUT",
    "DEVNULL",
    "run",
    "call",
    "check_call",
    "check_output",
    "getstatusoutput",
    "getoutput",
    "SubprocessError",
    "CalledProcessError",
    "TimeoutExpired",
    "CompletedProcess",
):
    print("in-all", name, name in subprocess.__all__)

print("exc-hier", issubclass(subprocess.CalledProcessError, subprocess.SubprocessError))
print("exc-hier2", issubclass(subprocess.TimeoutExpired, subprocess.SubprocessError))
print("exc-hier3", issubclass(subprocess.SubprocessError, Exception))
print("exc-mod", subprocess.SubprocessError.__module__, subprocess.CalledProcessError.__module__, subprocess.TimeoutExpired.__module__)

# --- CalledProcessError / TimeoutExpired / CompletedProcess ---
cpe = subprocess.CalledProcessError(2, ["cmd", "arg"], output=b"out", stderr=b"err")
print("cpe-attrs", cpe.returncode, cpe.cmd, cpe.output, cpe.stdout, cpe.stderr)
print("cpe-str", str(cpe))
cpe.stdout = b"updated"
print("cpe-stdout-alias", cpe.output, cpe.stdout)

cpe_sig = subprocess.CalledProcessError(-9, ["sleep"])
print("cpe-sig-str", str(cpe_sig))

te = subprocess.TimeoutExpired(["sleep", "5"], 0.5, output=b"part", stderr=b"epart")
print("te-attrs", te.cmd, te.timeout, te.output, te.stdout, te.stderr)
print("te-str", str(te))
te.stdout = b"part2"
print("te-stdout-alias", te.output, te.stdout)

cp0 = subprocess.CompletedProcess(["echo"], 0)
print("cp0-repr", repr(cp0))
print("cp0-check", cp0.check_returncode())
cp1 = subprocess.CompletedProcess(["false"], 3, stdout="o", stderr="e")
print("cp1-repr", repr(cp1))
try:
    cp1.check_returncode()
except subprocess.CalledProcessError as e:
    print("cp1-err", e.returncode, e.cmd, e.stdout, e.stderr)
print("cp-generic", subprocess.CompletedProcess[str].__origin__ is subprocess.CompletedProcess)
print("popen-generic", subprocess.Popen[bytes].__origin__ is subprocess.Popen)

# --- list2cmdline ---
print("l2c-basic", subprocess.list2cmdline(["a", "b c", "d\te", ""]))
print("l2c-quotes", subprocess.list2cmdline(['a"b', 'a\\b', 'a\\"b', 'a\\\\"b', 'trailing\\']))
print("l2c-pathlike", subprocess.list2cmdline([pathlib.Path("foo"), b"baz"]))

# --- run(): basic capture_output (bytes and text) ---
r_bytes = subprocess.run(
    [sys.executable, "-c", "import sys; sys.stdout.write('hello'); sys.stderr.write('oops')"],
    capture_output=True,
)
print("run-bytes", r_bytes.returncode, r_bytes.stdout, r_bytes.stderr, isinstance(r_bytes.stdout, bytes), isinstance(r_bytes.stderr, bytes))

r_text = subprocess.run(
    [sys.executable, "-c", "import sys; print('line1'); print('line2'); sys.stderr.write('warn\\n')"],
    capture_output=True,
    text=True,
)
print("run-text", r_text.returncode, repr(r_text.stdout), repr(r_text.stderr))

# universal_newlines=True alias and lone \r normalization
r_un = subprocess.run(
    [sys.executable, "-c", "import sys; sys.stdout.write('a\\rb\\n')"],
    stdout=subprocess.PIPE,
    universal_newlines=True,
)
print("run-un", r_un.returncode, repr(r_un.stdout), r_un.stderr)

# encoding + errors
r_enc = subprocess.run(
    [sys.executable, "-c", "import sys; sys.stdout.write('caf\\u00e9')"],
    stdout=subprocess.PIPE,
    encoding="utf-8",
)
print("run-utf8", r_enc.returncode, repr(r_enc.stdout))

r_err = subprocess.run(
    [sys.executable, "-c", "import sys; sys.stdout.write('ok\\u00e9')"],
    stdout=subprocess.PIPE,
    encoding="ascii",
    errors="replace",
)
print("run-ascii-replace", r_err.returncode, repr(r_err.stdout))

# --- run(): input (bytes, bytearray, text) ---
r_in_b = subprocess.run(
    [sys.executable, "-c", "import sys; sys.stdout.write(sys.stdin.read().upper())"],
    input=b"hello bytes",
    capture_output=True,
)
print("run-in-bytes", r_in_b.returncode, r_in_b.stdout)

r_in_ba = subprocess.run(
    [sys.executable, "-c", "import sys; sys.stdout.write(sys.stdin.read())"],
    input=bytearray(b"from_ba"),
    capture_output=True,
)
print("run-in-ba", r_in_ba.returncode, r_in_ba.stdout)

r_in_t = subprocess.run(
    [sys.executable, "-c", "import sys; sys.stdout.write(sys.stdin.read().upper())"],
    input="alpha\nbeta\n",
    capture_output=True,
    text=True,
)
print("run-in-text", r_in_t.returncode, repr(r_in_t.stdout))

# --- run(): stderr=STDOUT & DEVNULL ---
r_merged = subprocess.run(
    [
        sys.executable,
        "-c",
        "import sys; sys.stdout.write('OUT'); sys.stdout.flush(); sys.stderr.write('ERR'); sys.stderr.flush()",
    ],
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    text=True,
)
print("run-merged", r_merged.returncode, repr(r_merged.stdout), r_merged.stderr)

r_devnull = subprocess.run(
    [sys.executable, "-c", "import sys; data = sys.stdin.read(); print('noisy'); sys.stderr.write('err'); sys.exit(len(data))"],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
print("run-devnull", r_devnull.returncode, r_devnull.stdout, r_devnull.stderr)

# --- run(): check=True ---
r_ok = subprocess.run([sys.executable, "-c", "sys = __import__('sys'); sys.exit(0)"], check=True)
print("run-check-ok", r_ok.returncode)

try:
    subprocess.run(
        [sys.executable, "-c", "import sys; sys.stdout.write('o'); sys.stderr.write('e'); sys.exit(7)"],
        capture_output=True,
        text=True,
        check=True,
    )
    print("run-check-fail", "NO-ERROR")
except subprocess.CalledProcessError as e:
    print("run-check-fail", e.returncode, repr(e.stdout), repr(e.stderr))

# --- run(): timeout -> TimeoutExpired ---
try:
    subprocess.run(
        [sys.executable, "-c", "import time; print('before', flush=True); time.sleep(10)"],
        capture_output=True,
        text=True,
        timeout=0.25,
    )
    print("run-timeout", "NO-ERROR")
except subprocess.TimeoutExpired as e:
    print("run-timeout", e.timeout, isinstance(e.cmd, list))

# --- run(): cwd (str & PathLike) and env ---
with tempfile.TemporaryDirectory(prefix="pyrust_subproc_") as tmpdir:
    expected_cwd = os.path.realpath(tmpdir)

    r_cwd_str = subprocess.run(
        [sys.executable, "-c", "import os; print(os.path.realpath(os.getcwd()))"],
        cwd=tmpdir,
        capture_output=True,
        text=True,
    )
    print("run-cwd-str", r_cwd_str.returncode, r_cwd_str.stdout.strip() == expected_cwd)

    r_cwd_path = subprocess.run(
        [sys.executable, "-c", "import os; print(os.path.realpath(os.getcwd()))"],
        cwd=pathlib.Path(tmpdir),
        capture_output=True,
        text=True,
    )
    print("run-cwd-path", r_cwd_path.returncode, r_cwd_path.stdout.strip() == expected_cwd)

    custom_env = dict(os.environ)
    custom_env["PYRUST_SUBPROC_VAR"] = "magic_42"
    custom_env.pop("PYRUST_SUBPROC_ABSENT", None)
    r_env = subprocess.run(
        [
            sys.executable,
            "-c",
            "import os; print(os.environ.get('PYRUST_SUBPROC_VAR'), os.environ.get('PYRUST_SUBPROC_ABSENT'))",
        ],
        env=custom_env,
        capture_output=True,
        text=True,
    )
    print("run-env", r_env.returncode, r_env.stdout.strip())

    # Redirect stdout to an integer fd from tempfile.mkstemp
    fd, out_path = tempfile.mkstemp(prefix="sub_fd_", dir=tmpdir)
    try:
        r_fd = subprocess.run(
            [sys.executable, "-c", "import sys; sys.stdout.write('via_fd')"],
            stdout=fd,
        )
    finally:
        os.close(fd)
    with open(out_path, "rb") as rf:
        print("run-fd-stdout", r_fd.returncode, rf.read())

# --- args with PathLike and tuple ---
r_path_arg = subprocess.run(
    (pathlib.Path(sys.executable), "-c", "import sys; sys.stdout.write('tuple_path')"),
    capture_output=True,
    text=True,
)
print("run-tuple-path", r_path_arg.returncode, r_path_arg.stdout)

# --- shell=True ---
r_shell = subprocess.run("echo hello_shell", shell=True, capture_output=True, text=True)
print("run-shell", r_shell.returncode, r_shell.stdout.strip())

# --- FileNotFoundError ---
try:
    subprocess.run(["__pyrust_definitely_nonexistent_exe_3187__"])
    print("fnf-exe", "NO-ERROR")
except FileNotFoundError:
    print("fnf-exe", "FileNotFoundError")

try:
    subprocess.run([sys.executable, "-c", "pass"], cwd="__pyrust_definitely_nonexistent_dir_3187__")
    print("fnf-cwd", "NO-ERROR")
except (FileNotFoundError, NotADirectoryError) as e:
    print("fnf-cwd", type(e).__name__)

# --- Validation errors ---
for label, fn in [
    ("cap+stdout", lambda: subprocess.run([sys.executable, "-c", "pass"], capture_output=True, stdout=subprocess.PIPE)),
    ("cap+stderr", lambda: subprocess.run([sys.executable, "-c", "pass"], capture_output=True, stderr=subprocess.STDOUT)),
    ("stdin+input", lambda: subprocess.run([sys.executable, "-c", "pass"], stdin=subprocess.PIPE, input=b"x")),
    ("input-str-bin", lambda: subprocess.run([sys.executable, "-c", "pass"], input="abc")),
    ("input-bytes-txt", lambda: subprocess.run([sys.executable, "-c", "pass"], input=b"abc", text=True)),
    ("text-un-conflict", lambda: subprocess.Popen([sys.executable, "-c", "pass"], text=True, universal_newlines=False)),
    ("bad-bufsize", lambda: subprocess.Popen([sys.executable, "-c", "pass"], bufsize="no")),
    ("check_out+stdout", lambda: subprocess.check_output([sys.executable, "-c", "pass"], stdout=subprocess.PIPE)),
    ("check_out+check", lambda: subprocess.check_output([sys.executable, "-c", "pass"], check=False)),
]:
    try:
        fn()
        print("val", label, "NO-ERROR")
    except Exception as e:
        print("val", label, type(e).__name__, str(e))

# --- Popen: streaming I/O, poll, wait, context manager ---
with subprocess.Popen(
    [
        sys.executable,
        "-c",
        "import sys\nwhile True:\n    line = sys.stdin.readline()\n    if not line:\n        break\n    sys.stdout.write('ECHO:' + line)\n    sys.stdout.flush()\n",
    ],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
) as p:
    print("popen-pid", isinstance(p.pid, int) and p.pid > 0)
    print("popen-rc-before", p.returncode)
    print("popen-repr-prefix", repr(p).startswith("<Popen: returncode: None args: "))
    p.stdin.write("one\ntwo\n")
    p.stdin.flush()
    p.stdin.close()
    first = p.stdout.readline()
    rest = list(p.stdout)
    rc = p.wait()
    print("popen-stream", repr(first), rest, rc, p.poll(), p.returncode)
print("popen-closed-after-ctx", p.stdin.closed, p.stdout.closed)

# --- Popen: large bidirectional communicate (deadlock test > 128 KB) ---
big_payload = ("0123456789abcdef" * 8192)  # 131072 bytes
p_big = subprocess.Popen(
    [
        sys.executable,
        "-c",
        "import sys; d = sys.stdin.read(); sys.stdout.write(d); sys.stderr.write(d[:4096])",
    ],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
)
out_big, err_big = p_big.communicate(big_payload)
print("popen-big", p_big.returncode, len(out_big), out_big == big_payload, len(err_big), err_big == big_payload[:4096])

# --- Popen: timeout on wait/communicate, terminate, kill, send_signal ---
p_sleep = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"])
print("sleep-poll", p_sleep.poll())
try:
    p_sleep.wait(timeout=0.1)
    print("sleep-wait-timeout", "NO-ERROR")
except subprocess.TimeoutExpired as e:
    print("sleep-wait-timeout", e.timeout)
p_sleep.terminate()
rc_term = p_sleep.wait(timeout=5)
print("sleep-term-nonzero", rc_term != 0, p_sleep.poll() == rc_term)
# Terminating an already-exited process is a no-op
p_sleep.terminate()
p_sleep.kill()
p_sleep.send_signal(15)
print("sleep-post-noop", "ok")

p_kill = subprocess.Popen(
    [sys.executable, "-c", "import time; print('started', flush=True); time.sleep(10)"],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
)
try:
    p_kill.communicate(timeout=0.25)
    print("kill-comm-timeout", "NO-ERROR")
except subprocess.TimeoutExpired as e:
    print("kill-comm-timeout", e.timeout)
    p_kill.kill()
    out_k, err_k = p_kill.communicate(timeout=5)
    print("kill-comm-after", p_kill.returncode != 0, isinstance(out_k, str), isinstance(err_k, str))

# send_signal(15) (SIGTERM on both POSIX and Windows)
p_sig = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"])
p_sig.send_signal(15)
rc_sig = p_sig.wait(timeout=5)
print("send-sigterm-nonzero", rc_sig != 0)

# communicate() after stdout was manually closed -> ValueError
p_closed = subprocess.Popen(
    [sys.executable, "-c", "pass"],
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
)
p_closed.stdout.close()
try:
    p_closed.communicate()
    print("comm-closed", "NO-ERROR")
except ValueError:
    print("comm-closed", "ValueError")
finally:
    p_closed.wait()

# executable= override
r_exe = subprocess.run(
    ["ignored_prog", "-c", "import sys; sys.stdout.write('exe_ok')"],
    executable=sys.executable,
    capture_output=True,
    text=True,
)
print("run-executable", r_exe.returncode, r_exe.stdout)

# bytes elements in args list
r_bargs = subprocess.run(
    [sys.executable.encode("utf-8"), b"-c", b"import sys; sys.stdout.write('bargs_ok')"],
    capture_output=True,
    text=True,
)
print("run-bytes-args", r_bargs.returncode, r_bargs.stdout)

# incremental read(n) across multibyte UTF-8 and binary chunks + writelines
with subprocess.Popen(
    [sys.executable, "-c", "import sys; sys.stdout.write(sys.stdin.read())"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    encoding="utf-8",
) as p_utf:
    p_utf.stdin.writelines(["日本", "語 café!"])
    p_utf.stdin.close()
    chunks = []
    while True:
        c = p_utf.stdout.read(2)
        if not c:
            break
        chunks.append(c)
    p_utf.wait()
print("popen-utf8-chunks", chunks)

with subprocess.Popen(
    [sys.executable, "-c", "import sys; sys.stdout.write('abcdef')"],
    stdout=subprocess.PIPE,
) as p_bin:
    b1 = p_bin.stdout.read(2)
    b2 = p_bin.stdout.read(2)
    b3 = p_bin.stdout.read()
    p_bin.wait()
print("popen-bin-chunks", b1, b2, b3)

# embedded NUL byte validation
try:
    subprocess.run([sys.executable, "-c", "pass\x00bad"])
    print("nul-arg", "NO-ERROR")
except ValueError:
    print("nul-arg", "ValueError")

# --- call / check_call / check_output / getstatusoutput / getoutput ---
print("call-0", subprocess.call([sys.executable, "-c", "import sys; sys.exit(0)"]))
print("call-5", subprocess.call([sys.executable, "-c", "import sys; sys.exit(5)"]))
try:
    subprocess.call([sys.executable, "-c", "import time; time.sleep(10)"], timeout=0.1)
    print("call-timeout", "NO-ERROR")
except subprocess.TimeoutExpired:
    print("call-timeout", "TimeoutExpired")

print("check_call-0", subprocess.check_call([sys.executable, "-c", "import sys; sys.exit(0)"]))
try:
    subprocess.check_call([sys.executable, "-c", "import sys; sys.exit(4)"])
    print("check_call-4", "NO-ERROR")
except subprocess.CalledProcessError as e:
    print("check_call-4", e.returncode)

print(
    "check_output-bytes",
    subprocess.check_output([sys.executable, "-c", "import sys; sys.stdout.write('out_b')"]),
)
print(
    "check_output-text",
    repr(subprocess.check_output([sys.executable, "-c", "print('out_t')"], text=True)),
)
print(
    "check_output-input-none",
    repr(
        subprocess.check_output(
            [sys.executable, "-c", "import sys; sys.stdout.write('LEN:' + str(len(sys.stdin.read())))"],
            input=None,
            text=True,
        )
    ),
)
print(
    "check_output-merged",
    repr(
        subprocess.check_output(
            [sys.executable, "-c", "import sys; sys.stdout.write('A'); sys.stdout.flush(); sys.stderr.write('B'); sys.stderr.flush()"],
            stderr=subprocess.STDOUT,
            text=True,
        )
    ),
)
try:
    subprocess.check_output([sys.executable, "-c", "import sys; sys.stdout.write('partial'); sys.exit(9)"])
    print("check_output-fail", "NO-ERROR")
except subprocess.CalledProcessError as e:
    print("check_output-fail", e.returncode, e.output)

st, out_gso = subprocess.getstatusoutput("echo gso_ok")
print("gso", st, out_gso.strip())
print("go", subprocess.getoutput("echo go_ok").strip())
