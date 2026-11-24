"""Subprocesses with accessible I/O streams.

This module allows you to spawn processes, connect to their
input/output/error pipes, and obtain their return codes.
"""

import os as _os
import sys as _sys
from types import GenericAlias as _GenericAlias

_mswindows = _sys.platform == "win32"

PIPE = -1
STDOUT = -2
DEVNULL = -3

__all__ = [
    "Popen",
    "PIPE",
    "STDOUT",
    "call",
    "check_call",
    "getstatusoutput",
    "getoutput",
    "check_output",
    "run",
    "CalledProcessError",
    "DEVNULL",
    "SubprocessError",
    "TimeoutExpired",
    "CompletedProcess",
]

if _mswindows:
    CREATE_NEW_CONSOLE = 16
    CREATE_NEW_PROCESS_GROUP = 512
    STD_INPUT_HANDLE = 4294967286
    STD_OUTPUT_HANDLE = 4294967285
    STD_ERROR_HANDLE = 4294967284
    SW_HIDE = 0
    STARTF_USESTDHANDLES = 256
    STARTF_USESHOWWINDOW = 1
    ABOVE_NORMAL_PRIORITY_CLASS = 32768
    BELOW_NORMAL_PRIORITY_CLASS = 16384
    HIGH_PRIORITY_CLASS = 128
    IDLE_PRIORITY_CLASS = 64
    NORMAL_PRIORITY_CLASS = 32
    REALTIME_PRIORITY_CLASS = 256
    CREATE_NO_WINDOW = 134217728
    DETACHED_PROCESS = 8
    CREATE_DEFAULT_ERROR_MODE = 67108864
    CREATE_BREAKAWAY_FROM_JOB = 16777216

    class STARTUPINFO:
        def __init__(
            self,
            *,
            dwFlags=0,
            hStdInput=None,
            hStdOutput=None,
            hStdError=None,
            wShowWindow=0,
            lpAttributeList=None,
        ):
            self.dwFlags = dwFlags
            self.hStdInput = hStdInput
            self.hStdOutput = hStdOutput
            self.hStdError = hStdError
            self.wShowWindow = wShowWindow
            self.lpAttributeList = lpAttributeList or {"handle_list": []}

        def copy(self):
            attr_list = self.lpAttributeList.copy()
            if "handle_list" in attr_list:
                attr_list["handle_list"] = list(attr_list["handle_list"])
            return STARTUPINFO(
                dwFlags=self.dwFlags,
                hStdInput=self.hStdInput,
                hStdOutput=self.hStdOutput,
                hStdError=self.hStdError,
                wShowWindow=self.wShowWindow,
                lpAttributeList=attr_list,
            )

    __all__.extend(
        [
            "CREATE_NEW_CONSOLE",
            "CREATE_NEW_PROCESS_GROUP",
            "STD_INPUT_HANDLE",
            "STD_OUTPUT_HANDLE",
            "STD_ERROR_HANDLE",
            "SW_HIDE",
            "STARTF_USESTDHANDLES",
            "STARTF_USESHOWWINDOW",
            "STARTUPINFO",
            "ABOVE_NORMAL_PRIORITY_CLASS",
            "BELOW_NORMAL_PRIORITY_CLASS",
            "HIGH_PRIORITY_CLASS",
            "IDLE_PRIORITY_CLASS",
            "NORMAL_PRIORITY_CLASS",
            "REALTIME_PRIORITY_CLASS",
            "CREATE_NO_WINDOW",
            "DETACHED_PROCESS",
            "CREATE_DEFAULT_ERROR_MODE",
            "CREATE_BREAKAWAY_FROM_JOB",
        ]
    )


_POSIX_SIGNAL_NAMES = {
    1: "SIGHUP",
    2: "SIGINT",
    3: "SIGQUIT",
    4: "SIGILL",
    5: "SIGTRAP",
    6: "SIGABRT",
    7: "SIGBUS",
    8: "SIGFPE",
    9: "SIGKILL",
    10: "SIGUSR1",
    11: "SIGSEGV",
    12: "SIGUSR2",
    13: "SIGPIPE",
    14: "SIGALRM",
    15: "SIGTERM",
}

_WINDOWS_SIGNAL_NAMES = {
    2: "SIGINT",
    4: "SIGILL",
    8: "SIGFPE",
    11: "SIGSEGV",
    15: "SIGTERM",
    21: "SIGBREAK",
    22: "SIGABRT",
}

_SIGNAL_NAMES = _WINDOWS_SIGNAL_NAMES if _mswindows else _POSIX_SIGNAL_NAMES


class SubprocessError(Exception):
    pass


class CalledProcessError(SubprocessError):
    """Raised when run() is called with check=True and the process
    returns a non-zero exit status.

    Attributes:
      cmd, returncode, stdout, stderr, output
    """

    def __init__(self, returncode, cmd, output=None, stderr=None):
        self.returncode = returncode
        self.cmd = cmd
        self.output = output
        self.stderr = stderr

    def __str__(self):
        if self.returncode and self.returncode < 0:
            sig = -self.returncode
            name = _SIGNAL_NAMES.get(sig)
            if name is not None:
                return "Command '%s' died with <Signals.%s: %d>." % (
                    self.cmd,
                    name,
                    sig,
                )
            return "Command '%s' died with unknown signal %d." % (
                self.cmd,
                sig,
            )
        return "Command '%s' returned non-zero exit status %d." % (
            self.cmd,
            self.returncode,
        )

    @property
    def stdout(self):
        """Alias for output attribute, to match stderr"""
        return self.output

    @stdout.setter
    def stdout(self, value):
        self.output = value


class TimeoutExpired(SubprocessError):
    """This exception is raised when the timeout expires while waiting for a
    child process.

    Attributes:
        cmd, output, stdout, stderr, timeout
    """

    def __init__(self, cmd, timeout, output=None, stderr=None):
        self.cmd = cmd
        self.timeout = timeout
        self.output = output
        self.stderr = stderr

    def __str__(self):
        return "Command '%s' timed out after %s seconds" % (
            self.cmd,
            self.timeout,
        )

    @property
    def stdout(self):
        return self.output

    @stdout.setter
    def stdout(self, value):
        self.output = value


class CompletedProcess:
    """A process that has finished running.

    This is returned by run().

    Attributes:
      args: The list or str args passed to run().
      returncode: The exit code of the process, negative for signals.
      stdout: The standard output (None if not captured).
      stderr: The standard error (None if not captured).
    """

    def __init__(self, args, returncode, stdout=None, stderr=None):
        self.args = args
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr

    def __repr__(self):
        args = [
            "args={!r}".format(self.args),
            "returncode={!r}".format(self.returncode),
        ]
        if self.stdout is not None:
            args.append("stdout={!r}".format(self.stdout))
        if self.stderr is not None:
            args.append("stderr={!r}".format(self.stderr))
        return "{}({})".format(type(self).__name__, ", ".join(args))

    __class_getitem__ = classmethod(_GenericAlias)

    def check_returncode(self):
        """Raise CalledProcessError if the exit code is non-zero."""
        if self.returncode:
            raise CalledProcessError(
                self.returncode, self.args, self.stdout, self.stderr
            )


def _fsdecode(val):
    if isinstance(val, str):
        return val
    val = _os.fspath(val)
    if isinstance(val, str):
        return val
    return val.decode("utf-8", "surrogateescape")


def _is_pathlike(val):
    return hasattr(val, "__fspath__")


def _normalize_universal_newlines_str(s):
    if "\r" in s:
        return s.replace("\r\n", "\n").replace("\r", "\n")
    return s


def list2cmdline(seq):
    """Translate a sequence of arguments into a command line string, using
    the same rules as the MS C runtime.
    """
    result = []
    needquote = False
    for arg in map(_fsdecode, seq):
        bs_buf = []

        if result:
            result.append(" ")

        needquote = (" " in arg) or ("\t" in arg) or not arg
        if needquote:
            result.append('"')

        for c in arg:
            if c == "\\":
                bs_buf.append(c)
            elif c == '"':
                result.append("\\" * len(bs_buf) * 2)
                bs_buf = []
                result.append('\\"')
            else:
                if bs_buf:
                    result.extend(bs_buf)
                    bs_buf = []
                result.append(c)

        if bs_buf:
            result.extend(bs_buf)

        if needquote:
            result.extend(bs_buf)
            result.append('"')

    return "".join(result)


def _split_windows_cmdline(cmdline):
    """Split a Windows command string into (executable, raw_arg_tail)."""
    s = cmdline.lstrip(" \t")
    if not s:
        return ("", None)
    if s[0] == '"':
        end = s.find('"', 1)
        if end == -1:
            return (s[1:], None)
        prog = s[1:end]
        tail = s[end + 1 :].lstrip(" \t")
        return (prog, tail if tail else None)
    i = 0
    n = len(s)
    while i < n and s[i] not in (" ", "\t"):
        i += 1
    prog = s[:i]
    tail = s[i:].lstrip(" \t")
    return (prog, tail if tail else None)


def _resolve_stdio_spec(spec, is_stderr=False):
    if spec is None:
        return None
    if isinstance(spec, int) and not isinstance(spec, bool):
        if spec == PIPE or spec == DEVNULL:
            return spec
        if spec == STDOUT:
            if is_stderr:
                return STDOUT
            raise ValueError("STDOUT is only valid for stderr")
        if spec >= 0:
            return spec
        raise ValueError("Unsupported stdio specification: {}".format(spec))
    if hasattr(spec, "fileno"):
        fd = spec.fileno()
        if isinstance(fd, int) and fd >= 0:
            return fd
    raise TypeError(
        "Unsupported stdio object of type {}".format(type(spec).__name__)
    )


def _prepare_env(env):
    if env is None:
        return None
    if not hasattr(env, "items"):
        raise TypeError("env must be a mapping or None")
    pairs = []
    for k, v in env.items():
        if isinstance(k, bytes):
            if _mswindows:
                raise TypeError("environment can only contain strings")
            k = k.decode("utf-8", "surrogateescape")
        elif not isinstance(k, str):
            raise TypeError(
                "environment variable name must be str, not {}".format(
                    type(k).__name__
                )
            )
        if isinstance(v, bytes):
            if _mswindows:
                raise TypeError("environment can only contain strings")
            v = v.decode("utf-8", "surrogateescape")
        elif _is_pathlike(v):
            v = _fsdecode(v)
        elif not isinstance(v, str):
            raise TypeError(
                "environment variable value must be str, not {}".format(
                    type(v).__name__
                )
            )
        pairs.append((k, v))
    return pairs


def _prepare_command(args, executable, shell):
    """Build (program, argv, arg0, raw_args) for `_spawn_process`."""
    exe_str = _fsdecode(executable) if executable is not None else None

    if _mswindows:
        if isinstance(args, str):
            is_str_input = True
            str_cmd = args
        elif isinstance(args, bytes):
            if shell:
                raise TypeError("bytes args is not allowed on Windows")
            is_str_input = False
            arg_list = [_fsdecode(args)]
        elif _is_pathlike(args):
            if shell:
                raise TypeError(
                    "path-like args is not allowed when shell is true"
                )
            is_str_input = False
            arg_list = [_fsdecode(args)]
        else:
            is_str_input = False
            arg_list = [_fsdecode(a) for a in args]

        if shell:
            if not exe_str:
                comspec = _os.environ.get("ComSpec") or _os.environ.get(
                    "COMSPEC"
                )
                if not comspec:
                    sysroot = _os.environ.get("SystemRoot", "")
                    comspec = _os.path.join(sysroot, "System32", "cmd.exe")
                program = comspec
            else:
                program = exe_str
            cmd_str = str_cmd if is_str_input else list2cmdline(arg_list)
            raw_args = '/c "{}"'.format(cmd_str)
            return (program, [], None, raw_args)

        if is_str_input:
            if exe_str is not None:
                _, tail = _split_windows_cmdline(str_cmd)
                return (exe_str, [], None, tail)
            prog, tail = _split_windows_cmdline(str_cmd)
            if not prog:
                raise FileNotFoundError(
                    2, "The system cannot find the file specified", ""
                )
            return (prog, [], None, tail)

        if not arg_list:
            raise IndexError("list index out of range")
        program = exe_str if exe_str is not None else arg_list[0]
        return (program, arg_list[1:], None, None)

    # POSIX
    if isinstance(args, (str, bytes)) or _is_pathlike(args):
        arg_list = [_fsdecode(args)]
    else:
        arg_list = [_fsdecode(a) for a in args]

    if shell:
        unix_shell = exe_str if exe_str is not None else "/bin/sh"
        full = [unix_shell, "-c"] + arg_list
        return (unix_shell, full[1:], None, None)

    if not arg_list:
        raise IndexError("list index out of range")
    if exe_str is not None:
        return (exe_str, arg_list[1:], arg_list[0], None)
    return (arg_list[0], arg_list[1:], None, None)


class Popen:
    """Execute a child program in a new process."""

    def __init__(
        self,
        args,
        bufsize=-1,
        executable=None,
        stdin=None,
        stdout=None,
        stderr=None,
        preexec_fn=None,
        close_fds=True,
        shell=False,
        cwd=None,
        env=None,
        universal_newlines=None,
        startupinfo=None,
        creationflags=0,
        restore_signals=True,
        start_new_session=False,
        pass_fds=(),
        *,
        user=None,
        group=None,
        extra_groups=None,
        encoding=None,
        errors=None,
        text=None,
        umask=-1,
        pipesize=-1,
        process_group=None,
    ):
        if bufsize is None:
            bufsize = -1
        if not isinstance(bufsize, int):
            raise TypeError("bufsize must be an integer")

        if (
            text is not None
            and universal_newlines is not None
            and bool(universal_newlines) != bool(text)
        ):
            raise SubprocessError(
                "Cannot disambiguate when both text "
                "and universal_newlines are supplied but "
                "different. Pass one or the other."
            )

        self.args = args
        self.stdin = None
        self.stdout = None
        self.stderr = None
        self.pid = None
        self.returncode = None
        self.encoding = encoding
        self.errors = errors
        self.pipesize = pipesize

        self.text_mode = encoding or errors or text or universal_newlines
        text_mode_bool = bool(self.text_mode)
        if text_mode_bool and self.encoding is None:
            self.encoding = "utf-8"

        cwd_str = _fsdecode(cwd) if cwd is not None else None
        env_pairs = _prepare_env(env)
        stdin_spec = _resolve_stdio_spec(stdin, is_stderr=False)
        stdout_spec = _resolve_stdio_spec(stdout, is_stderr=False)
        stderr_spec = _resolve_stdio_spec(stderr, is_stderr=True)
        program, argv, arg0, raw_args = _prepare_command(
            args, executable, bool(shell)
        )

        handle = _spawn_process(
            program,
            argv,
            arg0,
            raw_args,
            cwd_str,
            env_pairs,
            stdin_spec,
            stdout_spec,
            stderr_spec,
            text_mode_bool,
            self.encoding,
            self.errors,
        )
        self._handle = handle
        self.pid = handle.pid
        self.stdin = handle.stdin
        self.stdout = handle.stdout
        self.stderr = handle.stderr

    __class_getitem__ = classmethod(_GenericAlias)

    @property
    def universal_newlines(self):
        return bool(self.text_mode)

    @universal_newlines.setter
    def universal_newlines(self, universal_newlines):
        self.text_mode = bool(universal_newlines)

    def __repr__(self):
        obj_repr = (
            f"<{self.__class__.__name__}: "
            f"returncode: {self.returncode} args: {self.args!r}>"
        )
        if len(obj_repr) > 80:
            obj_repr = obj_repr[:76] + "...>"
        return obj_repr

    def __enter__(self):
        return self

    def __exit__(self, exc_type, value, traceback):
        if self.stdout:
            self.stdout.close()
        if self.stderr:
            self.stderr.close()
        try:
            if self.stdin:
                self.stdin.close()
        finally:
            self.wait()

    def poll(self):
        if self.returncode is None:
            rc = self._handle.poll()
            if rc is not None:
                self.returncode = rc
        return self.returncode

    def wait(self, timeout=None):
        if self.returncode is not None:
            return self.returncode
        rc = self._handle.wait(timeout)
        if rc is None:
            raise TimeoutExpired(self.args, timeout)
        self.returncode = rc
        return rc

    def _encode_input(self, data):
        if data is None:
            return None
        if self.text_mode:
            if not isinstance(data, str):
                raise TypeError(
                    "write() argument must be str, not {}".format(
                        type(data).__name__
                    )
                )
            if _mswindows:
                data = data.replace("\n", "\r\n")
            enc = self.encoding or "utf-8"
            errs = self.errors or "strict"
            return data.encode(enc, errs)
        if isinstance(data, bytes):
            return data
        if isinstance(data, bytearray):
            return bytes(data)
        raise TypeError(
            "a bytes-like object is required, not '{}'".format(
                type(data).__name__
            )
        )

    def _decode_output(self, data):
        if data is None:
            return None
        if self.text_mode:
            enc = self.encoding or "utf-8"
            errs = self.errors or "strict"
            decoded = data.decode(enc, errs)
            return _normalize_universal_newlines_str(decoded)
        return data

    def communicate(self, input=None, timeout=None):
        if input is not None and self.stdin is not None:
            raw_input = self._encode_input(input)
        elif input is not None:
            # Validate input type even when stdin is not piped if text_mode
            # vs binary mode mismatches? CPython ignores input when stdin is
            # None, so keep raw_input = None.
            raw_input = None
        else:
            raw_input = None

        timed_out, out_b, err_b, rc = self._handle.communicate(
            raw_input, timeout
        )
        if timed_out:
            if _mswindows:
                raise TimeoutExpired(self.args, timeout)
            raise TimeoutExpired(
                self.args, timeout, output=out_b, stderr=err_b
            )
        if rc is not None:
            self.returncode = rc
        return (self._decode_output(out_b), self._decode_output(err_b))

    def send_signal(self, sig):
        if self.returncode is not None:
            return
        self._handle.send_signal(sig)

    def terminate(self):
        if self.returncode is not None:
            return
        self._handle.terminate()

    def kill(self):
        if self.returncode is not None:
            return
        self._handle.kill()


def call(*popenargs, timeout=None, **kwargs):
    """Run command with arguments.  Wait for command to complete or
    for timeout seconds, then return the returncode attribute.
    """
    with Popen(*popenargs, **kwargs) as p:
        try:
            return p.wait(timeout=timeout)
        except BaseException:
            p.kill()
            raise


def check_call(*popenargs, **kwargs):
    """Run command with arguments.  Wait for command to complete.  If
    the exit code was zero then return, otherwise raise
    CalledProcessError.
    """
    retcode = call(*popenargs, **kwargs)
    if retcode:
        cmd = kwargs.get("args")
        if cmd is None:
            cmd = popenargs[0]
        raise CalledProcessError(retcode, cmd)
    return 0


def check_output(*popenargs, timeout=None, **kwargs):
    """Run command with arguments and return its output.

    If the exit code was non-zero it raises a CalledProcessError.
    """
    for kw in ("stdout", "check"):
        if kw in kwargs:
            raise ValueError(
                f"{kw} argument not allowed, it will be overridden."
            )

    if "input" in kwargs and kwargs["input"] is None:
        if (
            kwargs.get("universal_newlines")
            or kwargs.get("text")
            or kwargs.get("encoding")
            or kwargs.get("errors")
        ):
            empty = ""
        else:
            empty = b""
        kwargs["input"] = empty

    return run(
        *popenargs, stdout=PIPE, timeout=timeout, check=True, **kwargs
    ).stdout


def run(
    *popenargs,
    input=None,
    capture_output=False,
    timeout=None,
    check=False,
    **kwargs,
):
    """Run command with arguments and return a CompletedProcess instance."""
    if input is not None:
        if kwargs.get("stdin") is not None:
            raise ValueError("stdin and input arguments may not both be used.")
        kwargs["stdin"] = PIPE

    if capture_output:
        if (
            kwargs.get("stdout") is not None
            or kwargs.get("stderr") is not None
        ):
            raise ValueError(
                "stdout and stderr arguments may not be used "
                "with capture_output."
            )
        kwargs["stdout"] = PIPE
        kwargs["stderr"] = PIPE

    with Popen(*popenargs, **kwargs) as process:
        try:
            stdout, stderr = process.communicate(input, timeout=timeout)
        except TimeoutExpired as exc:
            process.kill()
            if _mswindows:
                exc.stdout, exc.stderr = process.communicate()
            else:
                process.wait()
            raise
        except BaseException:
            process.kill()
            raise
        retcode = process.poll()
        if check and retcode:
            raise CalledProcessError(
                retcode, process.args, output=stdout, stderr=stderr
            )
    return CompletedProcess(process.args, retcode, stdout, stderr)


def getstatusoutput(cmd, *, encoding=None, errors=None):
    """Return (exitcode, output) of executing cmd in a shell."""
    try:
        data = check_output(
            cmd,
            shell=True,
            text=True,
            stderr=STDOUT,
            encoding=encoding,
            errors=errors,
        )
        exitcode = 0
    except CalledProcessError as ex:
        data = ex.output
        exitcode = ex.returncode
    if data[-1:] == "\n":
        data = data[:-1]
    return exitcode, data


def getoutput(cmd, *, encoding=None, errors=None):
    """Return output (stdout or stderr) of executing cmd in a shell."""
    return getstatusoutput(cmd, encoding=encoding, errors=errors)[1]
