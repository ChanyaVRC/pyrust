# Issue #3261: POSIX OS stdlib modules (`posix`, `fcntl`, `termios`, `tty`,
# `pty`, `pwd`, `grp`, `resource`, `syslog`) and POSIX-specific `os` syscalls.
#
# - On Windows (`sys.platform == "win32"`): verifies importing each POSIX-only
#   module raises `ModuleNotFoundError`, matching CPython 3.12.3.
# - On Unix (`os.name == "posix"`): exercises every POSIX module and `os`
#   syscall deterministically against CPython 3.12.3.

import os
import sys

POSIX_ONLY_MODULES = [
    "posix",
    "fcntl",
    "termios",
    "tty",
    "pty",
    "pwd",
    "grp",
    "resource",
    "syslog",
]

if sys.platform == "win32":
    print("platform:win32", os.name, os.supports_bytes_environ)
    for mod_name in POSIX_ONLY_MODULES:
        try:
            __import__(mod_name)
            print("UNEXPECTED_IMPORT", mod_name)
        except ModuleNotFoundError as exc:
            print("missing_ok", mod_name, type(exc).__name__)
    print(
        "win_absent",
        hasattr(os, "environb"),
        hasattr(os, "uname"),
        hasattr(os, "fork"),
        hasattr(os, "statvfs"),
        hasattr(os, "mkfifo"),
    )
else:
    import fcntl
    import grp
    import posix
    import pty
    import pwd
    import resource
    import stat
    import syslog
    import tempfile
    import termios
    import tty

    print("platform:posix", os.name, os.supports_bytes_environ)

    # ── 1. posix & POSIX os syscalls ────────────────────────────────────────
    u = os.uname()
    u_posix = posix.uname()
    print(
        "uname",
        isinstance(u, tuple),
        len(u),
        u == u_posix,
        type(u) is os.uname_result,
        os.uname_result is posix.uname_result,
        repr(u).startswith("posix.uname_result("),
        all(isinstance(x, str) and len(x) > 0 for x in u),
        (u.sysname, u.nodename, u.release, u.version, u.machine) == tuple(u),
    )

    uid = os.getuid()
    euid = os.geteuid()
    gid = os.getgid()
    egid = os.getegid()
    groups = os.getgroups()
    print(
        "ids",
        isinstance(uid, int) and uid >= 0,
        isinstance(euid, int) and euid >= 0,
        isinstance(gid, int) and gid >= 0,
        isinstance(egid, int) and egid >= 0,
        uid == posix.getuid() and gid == posix.getgid(),
        isinstance(groups, list) and all(isinstance(g, int) and g >= 0 for g in groups),
        groups == posix.getgroups(),
        os.getpgrp() == os.getpgid(0),
        os.getsid(0) > 0,
    )

    sv = os.statvfs("/")
    print(
        "statvfs",
        isinstance(sv, tuple),
        len(sv),
        sv.n_sequence_fields,
        sv.n_fields,
        type(sv) is os.statvfs_result,
        os.statvfs_result is posix.statvfs_result,
        repr(sv).startswith("os.statvfs_result("),
        sv.f_bsize > 0 and sv.f_frsize > 0 and sv.f_namemax > 0,
        isinstance(sv.f_fsid, int),
    )

    with tempfile.TemporaryDirectory() as tmpdir:
        fifo_path = os.path.join(tmpdir, "test_pipe")
        os.mkfifo(fifo_path, 0o600)
        fifo_st = os.stat(fifo_path)
        print("mkfifo", stat.S_ISFIFO(fifo_st.st_mode))

        data_path = os.path.join(tmpdir, "sample.txt")
        fd = os.open(data_path, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o600)
        try:
            os.write(fd, b"hello posix")
            os.chown(data_path, -1, -1)
            os.lchown(data_path, -1, -1)
            os.fchown(fd, -1, -1)
            fsv = os.fstatvfs(fd)
            print("chown_fstatvfs", type(fsv) is os.statvfs_result, fsv.f_namemax > 0)

            # pathconf / fpathconf / sysconf / confstr
            pc_path = os.pathconf(data_path, "PC_NAME_MAX")
            pc_fd = os.fpathconf(fd, "PC_NAME_MAX")
            sc_page = os.sysconf("SC_PAGESIZE")
            sc_clk = os.sysconf("SC_CLK_TCK")
            cs_path = os.confstr("CS_PATH")
            print(
                "conf",
                "PC_NAME_MAX" in os.pathconf_names,
                pc_path > 0 and pc_path == pc_fd,
                "SC_PAGESIZE" in os.sysconf_names,
                sc_page == resource.getpagesize(),
                sc_clk > 0,
                "CS_PATH" in os.confstr_names,
                isinstance(cs_path, str) and len(cs_path) > 0,
            )
            bad_conf = []
            for fn_call in (
                lambda: os.pathconf(data_path, "NO_SUCH_CONF"),
                lambda: os.sysconf("NO_SUCH_CONF"),
                lambda: os.confstr("NO_SUCH_CONF"),
            ):
                try:
                    fn_call()
                except ValueError:
                    bad_conf.append("ValueError")
            print("bad_conf", bad_conf)

            # ── 2. fcntl ───────────────────────────────────────────────────
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(fd, fcntl.LOCK_UN)
            fcntl.lockf(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.lockf(fd, fcntl.LOCK_UN)
            orig_fd_flags = fcntl.fcntl(fd, fcntl.F_GETFD)
            fcntl.fcntl(fd, fcntl.F_SETFD, orig_fd_flags | fcntl.FD_CLOEXEC)
            after_fd_flags = fcntl.fcntl(fd, fcntl.F_GETFD)
            fl_flags = fcntl.fcntl(fd, fcntl.F_GETFL)
            print(
                "fcntl",
                isinstance(orig_fd_flags, int),
                bool(after_fd_flags & fcntl.FD_CLOEXEC),
                isinstance(fl_flags, int),
                fcntl.F_RDLCK != fcntl.F_WRLCK,
            )

            # termios on non-tty raises termios.error (not OSError)
            try:
                termios.tcgetattr(fd)
                print("termios_nontty", "UNEXPECTED")
            except termios.error:
                print(
                    "termios_nontty",
                    issubclass(termios.error, Exception),
                    issubclass(termios.error, OSError),
                )
        finally:
            os.close(fd)

    # environb sync with environ
    env_key_s = "PYRUST_POSIX_TEST_3261"
    env_key_b = b"PYRUST_POSIX_TEST_3261"
    os.environb[env_key_b] = b"alpha"
    v_str = os.environ[env_key_s]
    v_bytes = os.getenvb(env_key_b)
    os.environ[env_key_s] = "beta"
    v_bytes2 = os.environb[env_key_b]
    has_before = env_key_b in os.environb
    del os.environb[env_key_b]
    has_after = env_key_b in os.environb
    dflt_b = os.getenvb(env_key_b, b"fallback")
    try:
        _ = os.environb[env_key_s]
        envb_type_err = False
    except TypeError:
        envb_type_err = True
    print(
        "environb",
        v_str,
        v_bytes,
        v_bytes2,
        has_before,
        has_after,
        dflt_b,
        envb_type_err,
        len(os.environb) == len(os.environ),
        os.fsencode("abc") == b"abc",
        os.fsdecode(b"abc") == "abc",
    )

    # misc posix helpers & EX_* constants
    load = os.getloadavg()
    print(
        "misc_posix",
        isinstance(load, tuple) and len(load) == 3 and all(isinstance(x, float) and x >= 0.0 for x in load),
        isinstance(os.ctermid(), str) and len(os.ctermid()) > 0,
        isinstance(os.nice(0), int),
        os.EX_OK,
        os.EX_USAGE,
        os.EX_CONFIG,
        os.WNOHANG > 0,
        os.WUNTRACED > 0,
        os.WCONTINUED > 0,
    )

    # fork + waitpid + wait + WIF* helpers
    sys.stdout.flush()
    pid = os.fork()
    if pid == 0:
        os._exit(42)
    wpid, wstatus = os.waitpid(pid, 0)
    print(
        "fork_waitpid",
        wpid == pid,
        os.WIFEXITED(wstatus),
        os.WEXITSTATUS(wstatus),
        os.WIFSIGNALED(wstatus),
        os.WIFSTOPPED(wstatus),
        os.WIFCONTINUED(wstatus),
        os.WCOREDUMP(wstatus),
        os.waitstatus_to_exitcode(wstatus),
    )

    sys.stdout.flush()
    pid2 = os.fork()
    if pid2 == 0:
        os._exit(7)
    wpid2, wstatus2 = os.wait()
    print("fork_wait", wpid2 == pid2, os.WIFEXITED(wstatus2), os.WEXITSTATUS(wstatus2))

    # ── 3, 4, 5. pty, termios, tty ──────────────────────────────────────────
    master_fd, slave_fd = pty.openpty()
    try:
        tty_dev = os.ttyname(slave_fd)
        attrs = termios.tcgetattr(slave_fd)
        termios.tcsetattr(slave_fd, termios.TCSANOW, attrs)
        termios.tcflush(slave_fd, termios.TCIOFLUSH)
        termios.tcdrain(slave_fd)
        termios.tcflow(slave_fd, termios.TCOON)
        termios.tcsetwinsize(slave_fd, (30, 100))
        ws = termios.tcgetwinsize(slave_fd)

        # fcntl.ioctl TIOCGWINSZ (both bytes and mutable bytearray)
        raw_ws = fcntl.ioctl(slave_fd, termios.TIOCGWINSZ, b"\x00" * 8)
        mut_ws = bytearray(8)
        rc_ws = fcntl.ioctl(slave_fd, termios.TIOCGWINSZ, mut_ws, True)

        # tty.cfmakeraw / tty.cfmakecbreak
        raw_mode = [attrs[0], attrs[1], attrs[2], attrs[3], attrs[4], attrs[5], list(attrs[6])]
        ret_raw = tty.cfmakeraw(raw_mode)
        cbreak_mode = [attrs[0], attrs[1], attrs[2], attrs[3], attrs[4], attrs[5], list(attrs[6])]
        ret_cb = tty.cfmakecbreak(cbreak_mode)

        orig_raw = tty.setraw(slave_fd)
        after_raw = termios.tcgetattr(slave_fd)
        orig_cb = tty.setcbreak(slave_fd)
        termios.tcsetattr(slave_fd, termios.TCSANOW, orig_raw)

        print(
            "pty_termios_tty",
            pty.STDIN_FILENO,
            pty.STDOUT_FILENO,
            pty.STDERR_FILENO,
            pty.CHILD,
            os.isatty(slave_fd),
            isinstance(tty_dev, str) and tty_dev.startswith("/dev/"),
            isinstance(attrs, list) and len(attrs) == 7,
            len(attrs[tty.CC]) == termios.NCCS,
            ws,
            isinstance(raw_ws, bytes) and len(raw_ws) == 8,
            rc_ws == 0 and bytes(mut_ws) == raw_ws,
            ret_raw is None,
            (raw_mode[tty.OFLAG] & termios.OPOST) == 0,
            (raw_mode[tty.CFLAG] & termios.CSIZE) == termios.CS8,
            (raw_mode[tty.LFLAG] & (termios.ECHO | termios.ICANON)) == 0,
            raw_mode[tty.CC][termios.VMIN],
            raw_mode[tty.CC][termios.VTIME],
            ret_cb is None,
            (cbreak_mode[tty.LFLAG] & (termios.ECHO | termios.ICANON)) == 0,
            isinstance(orig_raw, list) and len(orig_raw) == 7,
            isinstance(orig_cb, list) and len(orig_cb) == 7,
            (after_raw[tty.LFLAG] & termios.ICANON) == 0,
            after_raw[tty.CC][termios.VMIN] == 1 and after_raw[tty.CC][termios.VTIME] == 0,
        )
    finally:
        os.close(master_fd)
        os.close(slave_fd)

    # ── 6. pwd ──────────────────────────────────────────────────────────────
    pw = pwd.getpwuid(uid)
    pw_by_name = pwd.getpwnam(pw.pw_name)
    pw_all = pwd.getpwall()
    try:
        pwd.getpwnam("__pyrust_nonexistent_user_3261__")
        pw_missing = False
    except KeyError:
        pw_missing = True
    print(
        "pwd",
        isinstance(pw, pwd.struct_passwd),
        isinstance(pw, tuple),
        len(pw),
        pw.pw_uid == uid,
        pw == pw_by_name,
        isinstance(pw.pw_dir, str) and isinstance(pw.pw_shell, str),
        isinstance(pw_all, list) and any(e.pw_uid == uid for e in pw_all),
        pw_missing,
    )

    # ── 7. grp ──────────────────────────────────────────────────────────────
    gr = grp.getgrgid(gid)
    gr_by_name = grp.getgrnam(gr.gr_name)
    gr_all = grp.getgrall()
    try:
        grp.getgrnam("__pyrust_nonexistent_group_3261__")
        gr_missing = False
    except KeyError:
        gr_missing = True
    print(
        "grp",
        isinstance(gr, grp.struct_group),
        isinstance(gr, tuple),
        len(gr),
        gr.gr_gid == gid,
        gr == gr_by_name,
        isinstance(gr.gr_mem, list),
        isinstance(gr_all, list) and any(e.gr_gid == gid for e in gr_all),
        gr_missing,
    )

    # ── 8. resource ─────────────────────────────────────────────────────────
    ps = resource.getpagesize()
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (soft, hard))
    ru = resource.getrusage(resource.RUSAGE_SELF)
    ru_child = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        resource.getrlimit(-999)
        bad_rlim = False
    except ValueError:
        bad_rlim = True
    print(
        "resource",
        resource.error is OSError,
        isinstance(ps, int) and ps > 0 and (ps & (ps - 1)) == 0,
        isinstance(soft, int) and soft > 0 and isinstance(hard, int),
        isinstance(ru, resource.struct_rusage),
        isinstance(ru, tuple),
        len(ru),
        isinstance(ru.ru_utime, float) and ru.ru_utime >= 0.0,
        isinstance(ru.ru_stime, float) and ru.ru_stime >= 0.0,
        isinstance(ru.ru_maxrss, int),
        len(ru_child),
        bad_rlim,
    )

    # ── 9. syslog ───────────────────────────────────────────────────────────
    mask_info = syslog.LOG_MASK(syslog.LOG_INFO)
    upto_warn = syslog.LOG_UPTO(syslog.LOG_WARNING)
    syslog.openlog("pyrust_3261", syslog.LOG_PID, syslog.LOG_USER)
    old_mask = syslog.setlogmask(syslog.LOG_UPTO(syslog.LOG_ERR))
    check_mask = syslog.setlogmask(old_mask)
    syslog.closelog()
    print(
        "syslog",
        mask_info == (1 << syslog.LOG_INFO),
        upto_warn == ((1 << (syslog.LOG_WARNING + 1)) - 1),
        isinstance(old_mask, int),
        check_mask == syslog.LOG_UPTO(syslog.LOG_ERR),
        syslog.LOG_EMERG,
        syslog.LOG_DEBUG,
        syslog.LOG_USER > 0,
        syslog.LOG_LOCAL0 < syslog.LOG_LOCAL7,
    )
