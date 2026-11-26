"""CPython 3.12.3 parity tests for Windows OS modules (issue #3262):
`nt`, `msvcrt`, `winreg`, `winsound`, and Windows-specific `os` helpers.

- On Unix (`sys.platform != "win32"`): verifies importing `nt`, `msvcrt`,
  `winreg`, and `winsound` raises `ModuleNotFoundError`.
- On Windows (`sys.platform == "win32"`): exercises `nt`, Windows `os` helpers,
  `msvcrt`, `winreg` (read-only + isolated round-trip of all value types), and
  `winsound` (silent validation & `SND_PURGE` only — never emits audible sound).

All stdout is deterministic and never prints process-specific handles, PIDs,
timestamps, or temporary registry key names.
"""

import os
import sys
import tempfile

if sys.platform != "win32":
    for mod_name in ("nt", "msvcrt", "winreg", "winsound"):
        try:
            __import__(mod_name)
            raise AssertionError(f"expected ModuleNotFoundError for {mod_name}")
        except ModuleNotFoundError as exc:
            assert exc.name == mod_name, (mod_name, exc.name)
    print("unix_module_not_found_ok")
else:
    import msvcrt
    import nt
    import winreg
    import winsound

    # ── 1. nt module & Windows os helpers ────────────────────────────────────
    assert nt.__name__ == "nt"
    assert os.name == "nt"
    assert not hasattr(nt, "name")
    assert nt.error is OSError
    assert os.error is OSError
    assert nt.stat_result is os.stat_result
    assert nt.startfile is os.startfile
    assert nt.listdrives is os.listdrives
    assert nt.listvolumes is os.listvolumes
    assert nt.listmounts is os.listmounts
    assert nt.device_encoding is os.device_encoding
    assert isinstance(nt.environ, dict)
    assert len(nt.environ) > 0

    drives = os.listdrives()
    assert isinstance(drives, list) and len(drives) >= 1
    assert all(isinstance(d, str) and len(d) == 3 and d[1:] == ":\\" for d in drives)
    assert nt.listdrives() == drives

    volumes = os.listvolumes()
    assert isinstance(volumes, list) and len(volumes) >= 1
    assert all(isinstance(v, str) and v.startswith("\\\\?\\Volume{") and v.endswith("\\") for v in volumes)
    assert nt.listvolumes() == volumes

    mounts = os.listmounts(volumes[0])
    assert isinstance(mounts, list)
    assert all(isinstance(m, str) for m in mounts)

    cwd = os.getcwd()
    full = nt._getfullpathname(".")
    assert isinstance(full, str) and full.lower() == cwd.lower()
    final = nt._getfinalpathname(".")
    assert isinstance(final, str) and len(final) > 0
    vol_path = nt._getvolumepathname(cwd)
    assert isinstance(vol_path, str) and vol_path.endswith("\\")
    disk = nt._getdiskusage(cwd)
    assert isinstance(disk, tuple) and len(disk) == 2
    assert isinstance(disk[0], int) and isinstance(disk[1], int)
    assert disk[0] > 0 and 0 <= disk[1] <= disk[0]

    # device_encoding returns str or None (None for non-tty / invalid fd)
    enc99 = os.device_encoding(99999)
    assert enc99 is None
    for std_fd in (0, 1, 2):
        enc = os.device_encoding(std_fd)
        assert enc is None or isinstance(enc, str)

    # startfile on nonexistent path raises FileNotFoundError
    try:
        os.startfile(r"C:\PyRustDefinitelyMissingFile_3262_XYZ.nonexistent")
        raise AssertionError("expected FileNotFoundError")
    except FileNotFoundError:
        pass

    # os.add_dll_directory context manager + error handling
    tmpdir = tempfile.mkdtemp(prefix="pyrust_win_os_3262_")
    try:
        with os.add_dll_directory(tmpdir) as added:
            assert added.path == tmpdir
            assert repr(added).startswith("<AddedDllDirectory(")
            assert repr(added).endswith(")>")
        assert added.path is None
        assert repr(added) == "<AddedDllDirectory()>"

        # Double close raises TypeError
        try:
            added.close()
            raise AssertionError("expected TypeError on double close")
        except TypeError:
            pass

        # Relative path -> OSError; nonexistent absolute path -> FileNotFoundError
        try:
            os.add_dll_directory("relative_dir_3262")
            raise AssertionError("expected OSError")
        except OSError:
            pass
        try:
            os.add_dll_directory(os.path.join(tmpdir, "does_not_exist"))
            raise AssertionError("expected FileNotFoundError")
        except FileNotFoundError:
            pass
    finally:
        os.rmdir(tmpdir)

    print("nt_and_os_helpers_ok")

    # ── 2. msvcrt ────────────────────────────────────────────────────────────
    assert msvcrt.LK_UNLCK == 0
    assert msvcrt.LK_LOCK == 1
    assert msvcrt.LK_NBLCK == 2
    assert msvcrt.LK_RLCK == 3
    assert msvcrt.LK_NBRLCK == 4
    assert msvcrt.SEM_FAILCRITICALERRORS == 1
    assert msvcrt.SEM_NOGPFAULTERRORBOX == 2
    assert msvcrt.SEM_NOALIGNMENTFAULTEXCEPT == 4
    assert msvcrt.SEM_NOOPENFILEERRORBOX == 32768
    assert isinstance(msvcrt.CRT_ASSEMBLY_VERSION, str) and len(msvcrt.CRT_ASSEMBLY_VERSION) > 0

    assert msvcrt.heapmin() is None
    err_mode = msvcrt.GetErrorMode()
    assert isinstance(err_mode, int)
    prev_mode = msvcrt.SetErrorMode(err_mode)
    assert isinstance(prev_mode, int)
    msvcrt.SetErrorMode(prev_mode)

    # Console callables exist
    for fn_name in ("kbhit", "getch", "getwch", "getche", "getwche", "putch", "putwch", "ungetch", "ungetwch"):
        assert callable(getattr(msvcrt, fn_name)), fn_name

    # File descriptor helpers: get_osfhandle, setmode, locking
    with tempfile.TemporaryFile() as tf:
        fd = tf.fileno()
        handle = msvcrt.get_osfhandle(fd)
        assert isinstance(handle, int) and handle != 0 and handle != -1

        orig_mode = msvcrt.setmode(fd, os.O_BINARY)
        assert orig_mode in (os.O_BINARY, os.O_TEXT)
        prev_m = msvcrt.setmode(fd, os.O_TEXT)
        assert prev_m == os.O_BINARY
        prev_m2 = msvcrt.setmode(fd, orig_mode)
        assert prev_m2 == os.O_TEXT

        tf.write(b"0123456789abcdef")
        tf.flush()
        tf.seek(0)
        msvcrt.locking(fd, msvcrt.LK_NBLCK, 8)
        msvcrt.locking(fd, msvcrt.LK_UNLCK, 8)
        msvcrt.locking(fd, msvcrt.LK_LOCK, 4)
        msvcrt.locking(fd, msvcrt.LK_UNLCK, 4)

    # Invalid fd / handle / console arg errors
    for bad_call in (
        lambda: msvcrt.get_osfhandle(99999),
        lambda: msvcrt.setmode(99999, os.O_BINARY),
        lambda: msvcrt.locking(99999, msvcrt.LK_NBLCK, 1),
        lambda: msvcrt.open_osfhandle(-1, 0),
    ):
        try:
            bad_call()
            raise AssertionError("expected OSError for bad fd/handle")
        except OSError:
            pass

    for bad_console in (
        lambda: msvcrt.putch(b"ab"),
        lambda: msvcrt.putwch("ab"),
    ):
        try:
            bad_console()
            raise AssertionError("expected TypeError for multi-char console arg")
        except TypeError:
            pass

    print("msvcrt_ok")

    # ── 3. winreg ────────────────────────────────────────────────────────────
    assert winreg.error is OSError
    assert winreg.HKEY_CLASSES_ROOT == 0xFFFFFFFF80000000
    assert winreg.HKEY_CURRENT_USER == 0xFFFFFFFF80000001
    assert winreg.HKEY_LOCAL_MACHINE == 0xFFFFFFFF80000002
    assert winreg.HKEY_USERS == 0xFFFFFFFF80000003
    assert winreg.HKEY_PERFORMANCE_DATA == 0xFFFFFFFF80000004
    assert winreg.HKEY_CURRENT_CONFIG == 0xFFFFFFFF80000005
    assert winreg.HKEY_DYN_DATA == 0xFFFFFFFF80000006

    assert winreg.KEY_READ == 131097
    assert winreg.KEY_WRITE == 131078
    assert winreg.KEY_ALL_ACCESS == 983103
    assert winreg.KEY_EXECUTE == 131097
    assert winreg.KEY_QUERY_VALUE == 1
    assert winreg.KEY_SET_VALUE == 2
    assert winreg.KEY_CREATE_SUB_KEY == 4
    assert winreg.KEY_ENUMERATE_SUB_KEYS == 8
    assert winreg.KEY_NOTIFY == 16
    assert winreg.KEY_CREATE_LINK == 32
    assert winreg.KEY_WOW64_64KEY == 256
    assert winreg.KEY_WOW64_32KEY == 512

    assert winreg.REG_NONE == 0
    assert winreg.REG_SZ == 1
    assert winreg.REG_EXPAND_SZ == 2
    assert winreg.REG_BINARY == 3
    assert winreg.REG_DWORD == 4
    assert winreg.REG_DWORD_LITTLE_ENDIAN == 4
    assert winreg.REG_DWORD_BIG_ENDIAN == 5
    assert winreg.REG_LINK == 6
    assert winreg.REG_MULTI_SZ == 7
    assert winreg.REG_RESOURCE_LIST == 8
    assert winreg.REG_FULL_RESOURCE_DESCRIPTOR == 9
    assert winreg.REG_RESOURCE_REQUIREMENTS_LIST == 10
    assert winreg.REG_QWORD == 11
    assert winreg.REG_QWORD_LITTLE_ENDIAN == 11

    # Read-only query against standard Windows key
    with winreg.OpenKey(
        winreg.HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
    ) as cv_key:
        assert isinstance(cv_key, winreg.HKEYType)
        assert bool(cv_key) is True
        assert isinstance(cv_key.handle, int) and cv_key.handle > 0
        assert int(cv_key) == cv_key.handle
        assert cv_key == cv_key
        assert not (cv_key == cv_key.handle)

        prod_name, prod_type = winreg.QueryValueEx(cv_key, "ProductName")
        assert isinstance(prod_name, str) and len(prod_name) > 0
        assert prod_type == winreg.REG_SZ

        n_sub, n_val, mod_ft = winreg.QueryInfoKey(cv_key)
        assert isinstance(n_sub, int) and n_sub >= 0
        assert isinstance(n_val, int) and n_val >= 1
        assert isinstance(mod_ft, int) and mod_ft > 0

        refl = winreg.QueryReflectionKey(cv_key)
        assert isinstance(refl, bool)

    assert bool(cv_key) is False
    assert cv_key.handle == 0
    assert int(cv_key) == 0

    # Detach + CloseKey
    k_det = winreg.OpenKeyEx(
        winreg.HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
    )
    raw_h = k_det.Detach()
    assert isinstance(raw_h, int) and raw_h > 0
    assert bool(k_det) is False and k_det.handle == 0
    winreg.CloseKey(raw_h)

    # ConnectRegistry (local) + ExpandEnvironmentStrings
    with winreg.ConnectRegistry(None, winreg.HKEY_CURRENT_USER) as conn:
        assert isinstance(conn, winreg.HKEYType) and bool(conn)
    expanded = winreg.ExpandEnvironmentStrings("%SystemRoot%")
    assert isinstance(expanded, str) and len(expanded) > 0 and "%" not in expanded

    # Direct construction of HKEYType raises TypeError
    try:
        winreg.HKEYType()
        raise AssertionError("expected TypeError")
    except TypeError:
        pass

    # Error parity: FileNotFoundError & ERROR_NO_MORE_ITEMS (OSError)
    try:
        winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\PyRustNonExistentKey_3262_XYZ")
        raise AssertionError("expected FileNotFoundError")
    except FileNotFoundError:
        pass

    try:
        winreg.EnumKey(winreg.HKEY_CURRENT_USER, 999999)
        raise AssertionError("expected OSError")
    except OSError:
        pass

    # Full isolated round-trip of all value types under HKCU\Software\PyRustParityTest_<pid>
    test_subkey = f"Software\\PyRustParityTest_3262_{os.getpid()}"
    try:
        with winreg.CreateKeyEx(
            winreg.HKEY_CURRENT_USER,
            test_subkey,
            0,
            winreg.KEY_ALL_ACCESS,
        ) as rk:
            # All value types including Unicode, empty, None, max unsigned ints
            winreg.SetValueEx(rk, "v_sz", 0, winreg.REG_SZ, "hello 日本語 🚀")
            winreg.SetValueEx(rk, "v_sz_empty", 0, winreg.REG_SZ, "")
            winreg.SetValueEx(rk, "v_sz_none", 0, winreg.REG_SZ, None)
            winreg.SetValueEx(rk, "v_exp", 0, winreg.REG_EXPAND_SZ, r"%SystemRoot%\System32")
            winreg.SetValueEx(rk, "v_msz", 0, winreg.REG_MULTI_SZ, ["alpha", "beta", "γ"])
            winreg.SetValueEx(rk, "v_msz_empty", 0, winreg.REG_MULTI_SZ, [])
            winreg.SetValueEx(rk, "v_msz_none", 0, winreg.REG_MULTI_SZ, None)
            winreg.SetValueEx(rk, "v_dw", 0, winreg.REG_DWORD, 0x12345678)
            winreg.SetValueEx(rk, "v_dw_max", 0, winreg.REG_DWORD, 0xFFFFFFFF)
            winreg.SetValueEx(rk, "v_dw_none", 0, winreg.REG_DWORD, None)
            winreg.SetValueEx(rk, "v_dw_be", 0, winreg.REG_DWORD_BIG_ENDIAN, b"\x01\x02\x03\x04")
            winreg.SetValueEx(rk, "v_qw", 0, winreg.REG_QWORD, 0x123456789ABCDEF0)
            winreg.SetValueEx(rk, "v_qw_max", 0, winreg.REG_QWORD, 0xFFFFFFFFFFFFFFFF)
            winreg.SetValueEx(rk, "v_bin", 0, winreg.REG_BINARY, b"\x00\x01\x7f\xff")
            winreg.SetValueEx(rk, "v_bin_empty", 0, winreg.REG_BINARY, b"")
            winreg.SetValueEx(rk, "v_bin_none", 0, winreg.REG_BINARY, None)
            winreg.SetValueEx(rk, "v_none", 0, winreg.REG_NONE, None)
            winreg.SetValueEx(rk, "v_none_b", 0, winreg.REG_NONE, b"raw")

            winreg.FlushKey(rk)

            assert winreg.QueryValueEx(rk, "v_sz") == ("hello 日本語 🚀", winreg.REG_SZ)
            assert winreg.QueryValueEx(rk, "v_sz_empty") == ("", winreg.REG_SZ)
            assert winreg.QueryValueEx(rk, "v_sz_none") == ("", winreg.REG_SZ)
            assert winreg.QueryValueEx(rk, "v_exp") == (r"%SystemRoot%\System32", winreg.REG_EXPAND_SZ)
            assert winreg.QueryValueEx(rk, "v_msz") == (["alpha", "beta", "γ"], winreg.REG_MULTI_SZ)
            assert winreg.QueryValueEx(rk, "v_msz_empty") == ([], winreg.REG_MULTI_SZ)
            assert winreg.QueryValueEx(rk, "v_msz_none") == ([], winreg.REG_MULTI_SZ)
            assert winreg.QueryValueEx(rk, "v_dw") == (0x12345678, winreg.REG_DWORD)
            assert winreg.QueryValueEx(rk, "v_dw_max") == (0xFFFFFFFF, winreg.REG_DWORD)
            assert winreg.QueryValueEx(rk, "v_dw_none") == (0, winreg.REG_DWORD)
            assert winreg.QueryValueEx(rk, "v_dw_be") == (b"\x01\x02\x03\x04", winreg.REG_DWORD_BIG_ENDIAN)
            assert winreg.QueryValueEx(rk, "v_qw") == (0x123456789ABCDEF0, winreg.REG_QWORD)
            assert winreg.QueryValueEx(rk, "v_qw_max") == (0xFFFFFFFFFFFFFFFF, winreg.REG_QWORD)
            assert winreg.QueryValueEx(rk, "v_bin") == (b"\x00\x01\x7f\xff", winreg.REG_BINARY)
            assert winreg.QueryValueEx(rk, "v_bin_empty") == (None, winreg.REG_BINARY)
            assert winreg.QueryValueEx(rk, "v_bin_none") == (None, winreg.REG_BINARY)
            assert winreg.QueryValueEx(rk, "v_none") == (None, winreg.REG_NONE)
            assert winreg.QueryValueEx(rk, "v_none_b") == (b"raw", winreg.REG_NONE)

            # SetValue / QueryValue / EnumKey / EnumValue / DeleteValue / DeleteKey
            assert winreg.QueryValue(rk, "") == ""
            winreg.SetValue(rk, "child_sub", winreg.REG_SZ, "child_default")
            assert winreg.QueryValue(rk, "child_sub") == "child_default"
            assert winreg.EnumKey(rk, 0) == "child_sub"

            enumerated = {}
            idx = 0
            while True:
                try:
                    name, val, typ = winreg.EnumValue(rk, idx)
                    enumerated[name] = (val, typ)
                    idx += 1
                except OSError:
                    break
            assert enumerated["v_sz"] == ("hello 日本語 🚀", winreg.REG_SZ)
            assert enumerated["v_dw"] == (0x12345678, winreg.REG_DWORD)
            assert enumerated["v_qw_max"] == (0xFFFFFFFFFFFFFFFF, winreg.REG_QWORD)
            assert len(enumerated) == 18

            winreg.DeleteValue(rk, "v_none_b")
            try:
                winreg.QueryValueEx(rk, "v_none_b")
                raise AssertionError("expected FileNotFoundError after DeleteValue")
            except FileNotFoundError:
                pass

            n_sub, n_val, _ = winreg.QueryInfoKey(rk)
            assert n_sub == 1
            assert n_val == 17

            winreg.DeleteKeyEx(rk, "child_sub")
            n_sub2, _, _ = winreg.QueryInfoKey(rk)
            assert n_sub2 == 0
    finally:
        try:
            winreg.DeleteKey(winreg.HKEY_CURRENT_USER, test_subkey + "\\child_sub")
        except OSError:
            pass
        try:
            winreg.DeleteKey(winreg.HKEY_CURRENT_USER, test_subkey)
        except OSError:
            pass

    print("winreg_ok")

    # ── 4. winsound (silent checks only) ─────────────────────────────────────
    assert winsound.SND_ASYNC == 1
    assert winsound.SND_NODEFAULT == 2
    assert winsound.SND_MEMORY == 4
    assert winsound.SND_LOOP == 8
    assert winsound.SND_NOSTOP == 16
    assert winsound.SND_PURGE == 64
    assert winsound.SND_APPLICATION == 128
    assert winsound.SND_NOWAIT == 8192
    assert winsound.SND_ALIAS == 65536
    assert winsound.SND_FILENAME == 131072
    # Added in CPython 3.14 (also provided by PyRust): verify when present.
    if hasattr(winsound, "SND_SYNC"):
        assert winsound.SND_SYNC == 0
    if hasattr(winsound, "SND_SENTRY"):
        assert winsound.SND_SENTRY == 524288
    if hasattr(winsound, "SND_SYSTEM"):
        assert winsound.SND_SYSTEM == 2097152

    assert winsound.MB_OK == 0
    assert winsound.MB_ICONHAND == 16
    assert winsound.MB_ICONQUESTION == 32
    assert winsound.MB_ICONEXCLAMATION == 48
    assert winsound.MB_ICONASTERISK == 64

    # Beep frequency range validation (silent: raises ValueError before calling Win32)
    for bad_freq in (0, -100, 36, 32768, 100000):
        try:
            winsound.Beep(bad_freq, 10)
            raise AssertionError(f"expected ValueError for freq={bad_freq}")
        except ValueError:
            pass

    # PlaySound flag validation (silent: raises RuntimeError before calling Win32)
    try:
        winsound.PlaySound(b"dummy", winsound.SND_MEMORY | winsound.SND_ASYNC)
        raise AssertionError("expected RuntimeError for SND_MEMORY | SND_ASYNC")
    except RuntimeError:
        pass

    # Silent purge call
    assert winsound.PlaySound(None, winsound.SND_PURGE) is None
    assert callable(winsound.MessageBeep)

    print("winsound_ok")
