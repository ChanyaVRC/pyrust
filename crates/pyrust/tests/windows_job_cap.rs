//! Integration tests for parity harness child-process limits (#3022).
//!
//! - Cross-platform env-var validation runs everywhere.
//! - On Windows, tests observably exercise the Job Object memory cap,
//!   `QueryInformationJobObject` introspection inside a capped child,
//!   allocation-storm termination, wall-clock kill with zero orphaned
//!   descendants, and real `pyrust.exe` execution under the Job Object cap.

#[path = "support/child_limits.rs"]
mod child_limits;

use child_limits::{
    DEFAULT_MEM_MB, DEFAULT_TIMEOUT_SECS, MAX_OVERRIDE_VALUE, parse_mem_limit_bytes,
    parse_timeout_secs,
};

#[test]
fn env_var_parsing_validates_and_rejects_junk_and_overflow() {
    // Defaults when unset.
    assert_eq!(
        parse_mem_limit_bytes(None).unwrap(),
        DEFAULT_MEM_MB * 1024 * 1024
    );
    assert_eq!(parse_timeout_secs(None).unwrap(), DEFAULT_TIMEOUT_SECS);

    // Valid overrides.
    assert_eq!(parse_mem_limit_bytes(Some("1")).unwrap(), 1024 * 1024);
    assert_eq!(
        parse_mem_limit_bytes(Some("512")).unwrap(),
        512 * 1024 * 1024
    );
    assert_eq!(
        parse_mem_limit_bytes(Some("9999999")).unwrap(),
        MAX_OVERRIDE_VALUE * 1024 * 1024
    );
    assert_eq!(parse_timeout_secs(Some("1")).unwrap(), 1);
    assert_eq!(parse_timeout_secs(Some("300")).unwrap(), 300);
    assert_eq!(parse_timeout_secs(Some("9999999")).unwrap(), 9_999_999);

    // Junk, zero, negative, whitespace, and overflow values (including the
    // #3017 wrap-around payload 18014398509481985) must all be rejected loudly.
    let invalid_values = [
        "",
        " ",
        "0",
        "-1",
        "-5",
        "+10",
        " 10",
        "10 ",
        "abc",
        "128mb",
        "1.5",
        "10000000",
        "18014398509481985",
        "18446744073709551615",
        "99999999999999999999999999999",
    ];
    for bad in invalid_values {
        assert!(
            parse_mem_limit_bytes(Some(bad)).is_err(),
            "expected PYRUST_PARITY_MEM_MB={bad:?} to be rejected"
        );
        assert!(
            parse_timeout_secs(Some(bad)).is_err(),
            "expected PYRUST_PARITY_TIMEOUT_S={bad:?} to be rejected"
        );
    }
}

#[cfg(windows)]
mod windows_tests {
    use std::env;
    use std::fs;
    use std::mem::{size_of, zeroed};
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr;
    use std::thread;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, FALSE, STILL_ACTIVE};
    use windows_sys::Win32::System::JobObjects::{
        IsProcessInJob, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::child_limits::{
        ChildLimits, DEFAULT_MEM_MB, DEFAULT_TIMEOUT_SECS, build_windows_command_line,
        run_and_capture, run_and_capture_with_limits,
    };

    #[test]
    fn windows_command_line_quotes_spaces_and_quotes_correctly() {
        let wide = build_windows_command_line(
            std::path::Path::new(r"C:\Program Files\Python\python.exe"),
            &["-c", r#"print("a \"b\"\\")"#, "plain", ""],
        );
        assert_eq!(wide.last(), Some(&0));
        let rendered = String::from_utf16(&wide[..wide.len() - 1]).unwrap();
        assert_eq!(
            rendered,
            r#""C:\Program Files\Python\python.exe" -c "print(\"a \\\"b\\\"\\\\\")" plain """#
        );
    }

    const HELPER_GUARD_ENV: &str = "PYRUST_WIN_JOB_HELPER";

    fn self_exe() -> PathBuf {
        env::current_exe().expect("current_exe must resolve for integration test")
    }

    fn pyrust_exe() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_pyrust"))
    }

    fn is_process_alive(pid: u32) -> bool {
        // SAFETY: querying whether a PID is still active with minimal rights.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        // SAFETY: `handle` is valid and `exit_code` is writable.
        let ok = unsafe { GetExitCodeProcess(handle, &raw mut exit_code) };
        // SAFETY: `handle` is owned by this function.
        unsafe { CloseHandle(handle) };
        ok != 0 && exit_code == STILL_ACTIVE as u32
    }

    fn scratch_path(tag: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("pyrust_win_job_{tag}_{pid}_{nanos}.tmp"))
    }

    // ── Helper entrypoints invoked inside capped child processes ─────────────

    #[test]
    #[ignore = "child helper invoked by job_object_cap_is_observably_active_in_child"]
    fn helper_query_own_job_limits() {
        if env::var_os(HELPER_GUARD_ENV).is_none() {
            return;
        }
        let mut in_job: i32 = 0;
        // SAFETY: null job handle queries whether the current process is in any job.
        let ok = unsafe { IsProcessInJob(GetCurrentProcess(), ptr::null_mut(), &raw mut in_job) };
        assert_ne!(ok, 0, "IsProcessInJob failed");
        assert_ne!(in_job, 0, "child process is not assigned to a Job Object");

        // SAFETY: plain C struct zero-initialization.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        let mut returned_len: u32 = 0;
        // SAFETY: null job handle queries the immediate job of the calling process.
        let qok = unsafe {
            QueryInformationJobObject(
                ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                &raw mut returned_len,
            )
        };
        assert_ne!(qok, 0, "QueryInformationJobObject failed");

        let flags = info.BasicLimitInformation.LimitFlags;
        println!(
            "JOB_OBSERVED in_job={} has_proc_mem={} has_job_mem={} has_kill_on_close={} proc_limit={} job_limit={}",
            in_job != 0,
            (flags & JOB_OBJECT_LIMIT_PROCESS_MEMORY) != 0,
            (flags & JOB_OBJECT_LIMIT_JOB_MEMORY) != 0,
            (flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE) != 0,
            info.ProcessMemoryLimit,
            info.JobMemoryLimit,
        );
    }

    #[test]
    #[ignore = "child helper invoked by allocation_storm_is_killed_by_job_object_memory_cap"]
    fn helper_allocation_storm() {
        if env::var_os(HELPER_GUARD_ENV).is_none() {
            return;
        }
        // Commit 8 MiB chunks in a loop up to 512 MiB, touching every page so
        // the OS must charge real committed memory against the Job Object cap.
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        for i in 0..64usize {
            let mut buf = vec![0u8; 8 * 1024 * 1024];
            for page in (0..buf.len()).step_by(4096) {
                buf[page] = (i & 0xff) as u8;
            }
            chunks.push(buf);
        }
        std::hint::black_box(&chunks);
        println!("SURVIVED_STORM");
    }

    #[test]
    #[ignore = "child helper invoked by wall_clock_timeout_kills_child_and_all_descendants"]
    fn helper_spawn_grandchildren_and_hang() {
        if env::var_os(HELPER_GUARD_ENV).is_none() {
            return;
        }
        let pid_file = env::var("PYRUST_WIN_JOB_PID_FILE").expect("pid file env");
        let exe = self_exe();
        let mut pids = Vec::new();
        let mut children = Vec::new();
        for _ in 0..2 {
            let gc = Command::new(&exe)
                .args(["--ignored", "--exact", "windows_tests::helper_hang_forever"])
                .env(HELPER_GUARD_ENV, "1")
                .spawn()
                .expect("failed to spawn grandchild");
            pids.push(gc.id().to_string());
            children.push(gc);
        }
        fs::write(&pid_file, pids.join("\n")).expect("failed to write grandchild pids");
        println!("GRANDCHILDREN_SPAWNED {}", pids.join(","));
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    #[ignore = "grandchild helper invoked by helper_spawn_grandchildren_and_hang"]
    fn helper_hang_forever() {
        if env::var_os(HELPER_GUARD_ENV).is_none() {
            return;
        }
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    // ── Observable Windows Job Object verification tests ─────────────────────

    #[test]
    fn job_object_cap_is_observably_active_in_child() {
        let cap_mb = 192u64;
        let expected_bytes = cap_mb * 1024 * 1024;
        let limits = ChildLimits {
            mem_limit_bytes: expected_bytes,
            timeout_secs: 30,
        };
        let out = run_and_capture_with_limits(
            &self_exe(),
            &[
                "--ignored",
                "--exact",
                "windows_tests::helper_query_own_job_limits",
                "--nocapture",
            ],
            &[(HELPER_GUARD_ENV, "1")],
            limits,
        )
        .expect("helper_query_own_job_limits should succeed under 192 MiB cap");

        let expected_line = format!(
            "JOB_OBSERVED in_job=true has_proc_mem=true has_job_mem=true has_kill_on_close=true proc_limit={expected_bytes} job_limit={expected_bytes}"
        );
        assert!(
            out.contains(&expected_line),
            "child did not observe expected Job Object limits.\nexpected substring: {expected_line}\nactual output:\n{out}"
        );
    }

    #[test]
    fn allocation_storm_is_killed_by_job_object_memory_cap() {
        let limits = ChildLimits {
            mem_limit_bytes: 64 * 1024 * 1024,
            timeout_secs: 30,
        };
        let res = run_and_capture_with_limits(
            &self_exe(),
            &[
                "--ignored",
                "--exact",
                "windows_tests::helper_allocation_storm",
                "--nocapture",
            ],
            &[(HELPER_GUARD_ENV, "1")],
            limits,
        );
        let err = res.expect_err("allocation storm should be killed by 64 MiB Job Object cap");
        assert!(
            !err.contains("SURVIVED_STORM"),
            "child survived 512 MiB allocation storm under 64 MiB cap:\n{err}"
        );
        assert!(
            !err.contains("timed out"),
            "allocation storm should fail due to memory cap, not wall-clock timeout:\n{err}"
        );
    }

    #[test]
    fn wall_clock_timeout_kills_child_and_all_descendants_with_zero_orphans() {
        let pid_file = scratch_path("grandchildren");
        let pid_file_str = pid_file.to_str().expect("utf-8 temp path").to_string();
        let limits = ChildLimits {
            mem_limit_bytes: 256 * 1024 * 1024,
            timeout_secs: 2,
        };
        let started = Instant::now();
        let res = run_and_capture_with_limits(
            &self_exe(),
            &[
                "--ignored",
                "--exact",
                "windows_tests::helper_spawn_grandchildren_and_hang",
                "--nocapture",
            ],
            &[
                (HELPER_GUARD_ENV, "1"),
                ("PYRUST_WIN_JOB_PID_FILE", &pid_file_str),
            ],
            limits,
        );
        let elapsed = started.elapsed();

        let err = res.expect_err("hanging child must be killed at wall-clock timeout");
        assert!(
            err.contains("timed out after 2s"),
            "expected 2s timeout error, got:\n{err}"
        );
        assert!(
            elapsed < Duration::from_secs(15),
            "timeout kill took too long ({elapsed:?}), possible pipe/orphan hang"
        );

        let contents = fs::read_to_string(&pid_file)
            .expect("child should have written grandchild PIDs before timeout");
        let _ = fs::remove_file(&pid_file);
        let pids: Vec<u32> = contents
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().parse::<u32>().expect("numeric grandchild PID"))
            .collect();
        assert_eq!(pids.len(), 2, "expected 2 grandchild PIDs, got {pids:?}");

        // Poll briefly for OS process-table cleanup after TerminateJobObject +
        // CloseHandle, verifying 0 live descendants survive.
        let poll_deadline = Instant::now() + Duration::from_secs(2);
        let mut alive: Vec<u32>;
        loop {
            alive = pids
                .iter()
                .copied()
                .filter(|&pid| is_process_alive(pid))
                .collect();
            if alive.is_empty() || Instant::now() >= poll_deadline {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            alive.is_empty(),
            "orphaned grandchild process(es) survived Job Object kill: {alive:?}"
        );
    }

    #[test]
    fn pyrust_child_runs_cleanly_under_default_cap_and_aborts_on_tight_cap() {
        let ok_script = scratch_path("ok_py");
        fs::write(&ok_script, "print(sum(range(100)))\n").expect("write ok script");
        let ok_out = run_and_capture(&pyrust_exe(), &[ok_script.as_path()])
            .expect("pyrust.exe must run cleanly under default 4 GiB Windows Job Object cap");
        let _ = fs::remove_file(&ok_script);
        assert_eq!(ok_out.trim(), "4950");

        // Verify pyrust.exe (even debug builds with a 512 MiB reserved
        // interpreter stack!) runs under a 64 MiB Job Object commit cap for a
        // small script — proving Windows Job Object limits charge committed
        // bytes, not uncommitted stack reservation.
        let low_ok_script = scratch_path("low_ok_py");
        fs::write(&low_ok_script, "print('commit_not_reserve')\n").expect("write low_ok script");
        let low_limits = ChildLimits {
            mem_limit_bytes: 64 * 1024 * 1024,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        };
        let low_out =
            run_and_capture_with_limits(&pyrust_exe(), &[low_ok_script.as_path()], &[], low_limits)
                .expect("pyrust.exe small script should run under 64 MiB Job Object commit cap");
        let _ = fs::remove_file(&low_ok_script);
        assert_eq!(low_out.trim(), "commit_not_reserve");

        // Now run a deliberate Python allocation storm in pyrust.exe under the
        // same 64 MiB Job Object cap and prove it is killed rather than
        // exhausting host RAM.
        let storm_script = scratch_path("storm_py");
        fs::write(
            &storm_script,
            "chunks = []\nfor _ in range(256):\n    chunks.append(bytearray(4 * 1024 * 1024))\nprint('SURVIVED_PYRUST_STORM')\n",
        )
        .expect("write storm script");
        let storm_res = run_and_capture_with_limits(
            &pyrust_exe(),
            &[storm_script.as_path()],
            &[],
            ChildLimits {
                mem_limit_bytes: 64 * 1024 * 1024,
                timeout_secs: 30,
            },
        );
        let _ = fs::remove_file(&storm_script);
        let storm_err =
            storm_res.expect_err("pyrust.exe allocation storm must be stopped by 64 MiB cap");
        assert!(
            !storm_err.contains("SURVIVED_PYRUST_STORM"),
            "pyrust.exe survived 1 GiB allocation storm under 64 MiB cap:\n{storm_err}"
        );
        assert!(
            !storm_err.contains("timed out"),
            "pyrust.exe storm should hit memory cap, not timeout:\n{storm_err}"
        );

        // Sanity-check default constants match Linux parity harness defaults.
        assert_eq!(DEFAULT_MEM_MB, 4096);
        assert_eq!(DEFAULT_TIMEOUT_SECS, 120);
    }
}
