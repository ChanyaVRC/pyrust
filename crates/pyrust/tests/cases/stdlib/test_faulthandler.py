# Parity test for `faulthandler` stdlib module (CPython 3.12, issue #3246).
import faulthandler
import io
import os
import sys
import tempfile


# 1. enable / disable / is_enabled
was_enabled = faulthandler.is_enabled()
faulthandler.disable()
print("after_disable:", faulthandler.is_enabled())

tmp_fd, tmp_path = tempfile.mkstemp(suffix=".fh.log")
os.close(tmp_fd)
try:
    with open(tmp_path, "w+") as f:
        faulthandler.enable(file=f, all_threads=True)
        print("after_enable:", faulthandler.is_enabled())
        faulthandler.disable()
        print("after_disable_2:", faulthandler.is_enabled())

    # 2. dump_traceback writes live stack to file
    def inner_frame_fn(out_file, all_threads):
        faulthandler.dump_traceback(file=out_file, all_threads=all_threads)

    def outer_frame_fn(out_file, all_threads):
        inner_frame_fn(out_file, all_threads)

    with open(tmp_path, "w+") as f:
        outer_frame_fn(f, False)
    with open(tmp_path, "r") as f:
        dumped_single = f.read()

    print("has_stack_header:", "Stack (most recent call first):" in dumped_single)
    print("has_inner_frame:", "in inner_frame_fn" in dumped_single)
    print("has_outer_frame:", "in outer_frame_fn" in dumped_single)
    print("inner_before_outer:", dumped_single.index("in inner_frame_fn") < dumped_single.index("in outer_frame_fn"))

    with open(tmp_path, "w+") as f:
        outer_frame_fn(f, True)
    with open(tmp_path, "r") as f:
        dumped_all = f.read()
    print("has_thread_header:", "Current thread 0x" in dumped_all and "(most recent call first):" in dumped_all)

    # 3. File validation (StringIO lacks fileno -> UnsupportedOperation; negative fd -> ValueError)
    try:
        faulthandler.dump_traceback(file=io.StringIO())
        print("stringio_err: none")
    except io.UnsupportedOperation:
        print("stringio_err: UnsupportedOperation")

    try:
        faulthandler.dump_traceback(file=-1)
        print("neg_fd_err: none")
    except ValueError:
        print("neg_fd_err: ValueError")

    # 4. dump_traceback_later validation + cancel
    for bad in (0, -1, -0.5):
        try:
            faulthandler.dump_traceback_later(bad)
            print("bad_timeout_none:", bad)
        except ValueError:
            print("bad_timeout_valerr:", bad)

    with open(tmp_path, "w+") as f:
        faulthandler.dump_traceback_later(60.0, repeat=False, file=f, exit=False)
        faulthandler.cancel_dump_traceback_later()
        # Cancelling again is a safe no-op
        faulthandler.cancel_dump_traceback_later()
    print("later_cancel_ok: True")

    # 5. Unix-only register / unregister
    if sys.platform != "win32":
        import signal
        with open(tmp_path, "w+") as f:
            faulthandler.register(signal.SIGUSR1, file=f, all_threads=True, chain=True)
            print("unix_unregister_true:", faulthandler.unregister(signal.SIGUSR1))
            print("unix_unregister_false:", faulthandler.unregister(signal.SIGUSR1))
    else:
        print("win_no_register:", hasattr(faulthandler, "register"), hasattr(faulthandler, "unregister"))
finally:
    if was_enabled:
        faulthandler.enable()
    if os.path.exists(tmp_path):
        os.unlink(tmp_path)
