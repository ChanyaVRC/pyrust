//! End-to-end CLI integration tests for standard Python execution flags
//! (`-c`, `-m`, `-V`/`-VV`/`--version`, `--`, `-`, `--emit mir`, and usage
//! errors — issue #3142).

use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut path = env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!("pyrust_cli_{tag}_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&path).expect("create temporary test directory");
        Self(path)
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let full = self.0.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent directories");
        }
        std::fs::write(&full, content).expect("write fixture file");
        full
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_cli(cwd: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = Command::new(pyrust_bin());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args).output().expect("run pyrust")
}

fn run_cli_with_stdin(cwd: Option<&Path>, args: &[&str], stdin_data: &str) -> Output {
    let mut cmd = Command::new(pyrust_bin());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pyrust");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(stdin_data.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait pyrust")
}

fn stdout_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

// ── -c <command> [args...] ───────────────────────────────────────────────────

#[test]
fn dash_c_executes_command_with_cpython_metadata_and_argv() {
    let dir = TempDir::new("dash_c_meta");
    dir.write("local_helper.py", "ANSWER = 42\n");

    let output = run_cli(
        Some(dir.path()),
        &[
            "-c",
            "import sys, local_helper\n\
             def f(): pass\n\
             print(sys.argv)\n\
             print(repr(sys.path[0]))\n\
             print(__name__, repr(__package__), '__file__' in globals())\n\
             print(f.__code__.co_filename, sys._getframe().f_code.co_filename)\n\
             print(local_helper.ANSWER)\n",
            "first",
            "-V",
            "--unknown",
            "-m",
            "ignored",
        ],
    );

    assert!(
        output.status.success(),
        "pyrust -c failed:\n{}",
        stderr_str(&output)
    );
    assert_eq!(
        stdout_str(&output),
        "['-c', 'first', '-V', '--unknown', '-m', 'ignored']\n\
         ''\n\
         __main__ None False\n\
         <string> <string>\n\
         42\n"
    );
}

#[test]
fn dash_c_attached_form_and_empty_command() {
    let attached = run_cli(
        None,
        &["-cimport sys; print('ok', sys.argv)", "arg1", "--flag"],
    );
    assert!(
        attached.status.success(),
        "attached -c failed:\n{}",
        stderr_str(&attached)
    );
    assert_eq!(stdout_str(&attached), "ok ['-c', 'arg1', '--flag']\n");

    let empty = run_cli(None, &["-c", ""]);
    assert!(
        empty.status.success(),
        "empty -c failed: {}",
        stderr_str(&empty)
    );
    assert!(stdout_str(&empty).is_empty());
}

#[test]
fn dash_c_errors_and_system_exit() {
    let exc = run_cli(None, &["-c", "raise ValueError('boom')"]);
    assert_eq!(exc.status.code(), Some(1));
    let err = stderr_str(&exc);
    assert!(
        err.contains("File \"<string>\", line 1, in <module>"),
        "expected <string> traceback frame:\n{err}"
    );
    assert!(
        err.contains("ValueError: boom"),
        "expected ValueError message:\n{err}"
    );

    let syn = run_cli(None, &["-c", "def broken("]);
    assert_eq!(syn.status.code(), Some(1));
    assert!(
        stderr_str(&syn).starts_with("SyntaxError:"),
        "expected SyntaxError:\n{}",
        stderr_str(&syn)
    );

    let exit_code = run_cli(None, &["-c", "import sys; print('bye'); sys.exit(42)"]);
    assert_eq!(exit_code.status.code(), Some(42));
    assert_eq!(stdout_str(&exit_code), "bye\n");
    assert!(stderr_str(&exit_code).is_empty());

    let exit_zero = run_cli(None, &["-c", "raise SystemExit(0)"]);
    assert_eq!(exit_zero.status.code(), Some(0));
}

// ── -m <module> [args...] ────────────────────────────────────────────────────

#[test]
fn dash_m_runs_top_level_module_and_attached_form() {
    let dir = TempDir::new("dash_m_top");
    let mod_path = dir.write(
        "topmod.py",
        "import sys\n\
         print(sys.argv[0] == __file__)\n\
         print(sys.argv[1:])\n\
         print(sys.path[0])\n\
         print(__name__, repr(__package__))\n\
         print('__main__' in sys.modules, 'topmod' in sys.modules)\n",
    );

    let output = run_cli(Some(dir.path()), &["-m", "topmod", "a", "--flag", "-V"]);
    assert!(
        output.status.success(),
        "pyrust -m topmod failed:\n{}",
        stderr_str(&output)
    );
    let lines: Vec<String> = stdout_str(&output).lines().map(String::from).collect();
    assert_eq!(
        lines.len(),
        5,
        "unexpected stdout:\n{}",
        stdout_str(&output)
    );
    assert_eq!(lines[0], "True");
    assert_eq!(lines[1], "['a', '--flag', '-V']");
    assert_eq!(
        std::fs::canonicalize(&lines[2]).expect("canonicalize sys.path[0]"),
        std::fs::canonicalize(dir.path()).expect("canonicalize cwd")
    );
    assert_eq!(lines[3], "__main__ ''");
    assert_eq!(lines[4], "True False");

    let attached = run_cli(Some(dir.path()), &["-mtopmod", "x"]);
    assert!(
        attached.status.success(),
        "attached -m failed:\n{}",
        stderr_str(&attached)
    );
    let attached_lines: Vec<String> = stdout_str(&attached).lines().map(String::from).collect();
    assert_eq!(attached_lines[1], "['x']");
    assert!(mod_path.is_file());
}

#[test]
fn dash_m_runs_dotted_submodule_with_relative_imports() {
    let dir = TempDir::new("dash_m_dotted");
    dir.write(
        "mypkg/__init__.py",
        "import sys\n\
         INIT_VAL = 10\n\
         print('init', __name__, repr(__package__), sys.argv)\n",
    );
    dir.write("mypkg/helper.py", "HELPER_VAL = 32\n");
    dir.write(
        "mypkg/sub.py",
        "import sys\n\
         from . import INIT_VAL\n\
         from .helper import HELPER_VAL\n\
         print('sub', __name__, repr(__package__), __file__.endswith('sub.py'))\n\
         print('argv', sys.argv[0] == __file__, sys.argv[1:])\n\
         print('vals', INIT_VAL + HELPER_VAL, 'mypkg' in sys.modules, 'mypkg.sub' in sys.modules)\n",
    );

    let output = run_cli(Some(dir.path()), &["-m", "mypkg.sub", "arg1", "--opt"]);
    assert!(
        output.status.success(),
        "pyrust -m mypkg.sub failed:\n{}",
        stderr_str(&output)
    );
    assert_eq!(
        stdout_str(&output),
        "init mypkg 'mypkg' ['-m', 'arg1', '--opt']\n\
         sub __main__ 'mypkg' True\n\
         argv True ['arg1', '--opt']\n\
         vals 42 True False\n"
    );
}

#[test]
fn dash_m_runs_package_and_nested_subpackage_dunder_main_with_relative_imports() {
    let dir = TempDir::new("dash_m_pkg_main");
    dir.write("app/__init__.py", "ROOT_VAL = 'root'\n");
    dir.write("app/util.py", "UTIL_VAL = 'util'\n");
    dir.write(
        "app/__main__.py",
        "import sys\n\
         from . import ROOT_VAL\n\
         from .util import UTIL_VAL\n\
         print(__name__, repr(__package__), __file__.endswith('__main__.py'))\n\
         print(sys.argv[0] == __file__, sys.argv[1:])\n\
         print(ROOT_VAL, UTIL_VAL, 'app' in sys.modules, 'app.__main__' in sys.modules)\n",
    );
    dir.write("app/sub/__init__.py", "SUB_VAL = 'sub'\n");
    dir.write(
        "app/sub/__main__.py",
        "from . import SUB_VAL\n\
         from .. import ROOT_VAL\n\
         print(__name__, repr(__package__), ROOT_VAL, SUB_VAL)\n",
    );

    let top_pkg = run_cli(Some(dir.path()), &["-m", "app", "go"]);
    assert!(
        top_pkg.status.success(),
        "pyrust -m app failed:\n{}",
        stderr_str(&top_pkg)
    );
    assert_eq!(
        stdout_str(&top_pkg),
        "__main__ 'app' True\n\
         True ['go']\n\
         root util True False\n"
    );

    let nested_pkg = run_cli(Some(dir.path()), &["-m", "app.sub"]);
    assert!(
        nested_pkg.status.success(),
        "pyrust -m app.sub failed:\n{}",
        stderr_str(&nested_pkg)
    );
    assert_eq!(stdout_str(&nested_pkg), "__main__ 'app.sub' root sub\n");
}

#[test]
fn dash_m_error_cases_match_cpython() {
    let dir = TempDir::new("dash_m_errors");
    dir.write("nomain/__init__.py", "X = 1\n");
    dir.write("plain.py", "Y = 2\n");
    dir.write(
        "badinit/__init__.py",
        "raise RuntimeError('init exploded')\n",
    );
    dir.write("badinit/__main__.py", "print('never')\n");
    dir.write("exitinit/__init__.py", "import sys; sys.exit(9)\n");
    dir.write("exitinit/__main__.py", "print('never')\n");

    let no_main = run_cli(Some(dir.path()), &["-m", "nomain"]);
    assert_eq!(no_main.status.code(), Some(1));
    let no_main_err = stderr_str(&no_main);
    assert!(
        no_main_err.contains("No module named nomain.__main__")
            && no_main_err.contains("'nomain' is a package and cannot be directly executed"),
        "unexpected stderr for package without __main__.py:\n{no_main_err}"
    );

    let missing = run_cli(Some(dir.path()), &["-m", "does_not_exist_xyz"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        stderr_str(&missing).contains("No module named does_not_exist_xyz"),
        "unexpected stderr for missing module:\n{}",
        stderr_str(&missing)
    );

    let not_pkg = run_cli(Some(dir.path()), &["-m", "plain.sub"]);
    assert_eq!(not_pkg.status.code(), Some(1));
    assert!(
        stderr_str(&not_pkg).contains("'plain' is not a package"),
        "unexpected stderr for non-package parent:\n{}",
        stderr_str(&not_pkg)
    );

    let rel = run_cli(Some(dir.path()), &["-m", ".relative"]);
    assert_eq!(rel.status.code(), Some(1));
    assert!(
        stderr_str(&rel).contains("Relative module names not supported"),
        "unexpected stderr for relative module name:\n{}",
        stderr_str(&rel)
    );

    let builtin = run_cli(Some(dir.path()), &["-m", "math"]);
    assert_eq!(builtin.status.code(), Some(1));
    assert!(
        stderr_str(&builtin).contains("No code object available for math"),
        "unexpected stderr for builtin module:\n{}",
        stderr_str(&builtin)
    );

    let bad_init = run_cli(Some(dir.path()), &["-m", "badinit"]);
    assert_eq!(bad_init.status.code(), Some(1));
    let bad_init_err = stderr_str(&bad_init);
    assert!(
        bad_init_err.contains("RuntimeError: init exploded"),
        "expected RuntimeError traceback from __init__.py:\n{bad_init_err}"
    );

    let exit_init = run_cli(Some(dir.path()), &["-m", "exitinit"]);
    assert_eq!(exit_init.status.code(), Some(9));
}

#[test]
fn dash_m_timeit_executes_benchmark_cli() {
    let output = run_cli(None, &["-m", "timeit", "-n", "10", "-r", "3", "1+1"]);
    assert!(
        output.status.success(),
        "pyrust -m timeit failed:\n{}",
        stderr_str(&output)
    );
    let out = stdout_str(&output);
    assert!(
        out.contains("10 loops, best of 3:") && out.contains("per loop"),
        "unexpected timeit CLI output:\n{out}"
    );
}

// ── -V / -VV / --version / -h ────────────────────────────────────────────────

#[test]
fn version_and_help_flags_print_to_stdout_and_exit_zero() {
    let v_short = run_cli(None, &["-V"]);
    assert_eq!(v_short.status.code(), Some(0));
    assert!(stderr_str(&v_short).is_empty());
    let v_short_out = stdout_str(&v_short);
    assert!(
        v_short_out.starts_with("Python 3.12") && v_short_out.contains("PyRust"),
        "unexpected -V output: {v_short_out:?}"
    );

    let v_long = run_cli(None, &["--version"]);
    assert_eq!(v_long.status.code(), Some(0));
    assert_eq!(stdout_str(&v_long), v_short_out);

    let v_verbose = run_cli(None, &["-VV"]);
    assert_eq!(v_verbose.status.code(), Some(0));
    assert!(stdout_str(&v_verbose).len() > v_short_out.len());

    let v_precedence = run_cli(None, &["-V", "-c", "raise RuntimeError('must not run')"]);
    assert_eq!(v_precedence.status.code(), Some(0));
    assert_eq!(stdout_str(&v_precedence), v_short_out);
    assert!(stderr_str(&v_precedence).is_empty());

    let help = run_cli(None, &["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(stdout_str(&help).contains("-c cmd"));
    assert!(stdout_str(&help).contains("-m mod"));
}

// ── -- terminator, - stdin, script args, --emit mir, usage errors ────────────

#[test]
fn double_dash_executes_dash_prefixed_script_file() {
    let dir = TempDir::new("double_dash");
    dir.write("-weird-file.py", "import sys\nprint(sys.argv, __file__)\n");

    let output = run_cli(Some(dir.path()), &["--", "-weird-file.py", "--after"]);
    assert!(
        output.status.success(),
        "pyrust -- -weird-file.py failed:\n{}",
        stderr_str(&output)
    );
    assert_eq!(
        stdout_str(&output),
        "['-weird-file.py', '--after'] -weird-file.py\n"
    );
}

#[test]
fn dash_reads_and_executes_stdin() {
    let output = run_cli_with_stdin(
        None,
        &["-", "one", "--two"],
        "import sys\nprint(sys.argv, repr(sys.path[0]), __name__, '__file__' in globals())\n",
    );
    assert!(
        output.status.success(),
        "pyrust - failed:\n{}",
        stderr_str(&output)
    );
    assert_eq!(
        stdout_str(&output),
        "['-', 'one', '--two'] '' __main__ False\n"
    );
}

#[test]
fn script_file_stops_flag_parsing_and_emit_mir_still_works() {
    let dir = TempDir::new("script_and_mir");
    let script = dir.write("prog.py", "import sys\nprint(sys.argv[1:])\n");

    let script_out = run_cli(
        None,
        &[script.to_str().unwrap(), "-c", "not-a-cmd", "-V", "--foo"],
    );
    assert!(
        script_out.status.success(),
        "script failed:\n{}",
        stderr_str(&script_out)
    );
    assert_eq!(
        stdout_str(&script_out),
        "['-c', 'not-a-cmd', '-V', '--foo']\n"
    );

    let mir_script = dir.write("simple.py", "x = 1 + 2\n");
    let mir_out = run_cli(None, &["--emit", "mir", mir_script.to_str().unwrap()]);
    assert!(
        mir_out.status.success(),
        "--emit mir failed:\n{}",
        stderr_str(&mir_out)
    );
    assert!(!stdout_str(&mir_out).is_empty());

    let mir_eq = run_cli(None, &["--emit=mir", mir_script.to_str().unwrap()]);
    assert!(
        mir_eq.status.success(),
        "--emit=mir failed:\n{}",
        stderr_str(&mir_eq)
    );
    assert_eq!(stdout_str(&mir_eq), stdout_str(&mir_out));
}

#[test]
fn missing_option_arguments_and_unknown_flags_exit_with_code_2() {
    for (args, expected) in [
        (&["-c"][..], "Argument expected for the -c option"),
        (&["-m"][..], "Argument expected for the -m option"),
        (&["-Z"][..], "Unknown option: -Z"),
        (&["--unknown-flag"][..], "Unknown option: --unknown-flag"),
    ] {
        let output = run_cli(None, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "expected exit code 2 for {args:?}, stderr:\n{}",
            stderr_str(&output)
        );
        assert!(
            stderr_str(&output).contains(expected),
            "expected {expected:?} in stderr for {args:?}:\n{}",
            stderr_str(&output)
        );
    }
}

// ── W5-02 (#3243): CPython 3.12 CLI flags, PYTHON* env vars, sys.flags & site ─

fn run_cli_with_env(cwd: Option<&Path>, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(pyrust_bin());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // Clear PYTHON* vars that might leak from the host environment.
    for key in [
        "PYTHONDEBUG",
        "PYTHONDEVMODE",
        "PYTHONDONTWRITEBYTECODE",
        "PYTHONHASHSEED",
        "PYTHONHOME",
        "PYTHONINSPECT",
        "PYTHONINTMAXSTRDIGITS",
        "PYTHONNOUSERSITE",
        "PYTHONOPTIMIZE",
        "PYTHONPATH",
        "PYTHONPLATLIBDIR",
        "PYTHONPYCACHEPREFIX",
        "PYTHONSAFEPATH",
        "PYTHONUNBUFFERED",
        "PYTHONUTF8",
        "PYTHONVERBOSE",
        "PYTHONWARNDEFAULTENCODING",
        "PYTHONWARNINGS",
    ] {
        cmd.env_remove(key);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.args(args).output().expect("run pyrust")
}

#[test]
fn optimize_flags_o_oo_and_pythonoptimize() {
    let script = r#"
"""module docstring"""
import builtins, sys

def fn():
    """fn docstring"""
    assert False, "should be stripped"
    return 1

class C:
    """class docstring"""

c_code = compile("assert False\nx = __debug__\n", "<dyn>", "exec")
ns = {}
exec(c_code, ns)

print(sys.flags.optimize, __debug__, builtins.__debug__, fn(), __doc__, fn.__doc__, C.__doc__, ns["x"])
"#;

    // -O: optimize=1, __debug__=False, assert omitted, docstrings KEPT.
    let o1 = run_cli_with_env(None, &["-O", "-c", script], &[]);
    assert!(o1.status.success(), "-O failed:\n{}", stderr_str(&o1));
    assert_eq!(
        stdout_str(&o1),
        "1 False False 1 module docstring fn docstring class docstring False\n"
    );

    // -OO: optimize=2, __debug__=False, assert omitted, docstrings STRIPPED.
    let o2 = run_cli_with_env(None, &["-OO", "-c", script], &[]);
    assert!(o2.status.success(), "-OO failed:\n{}", stderr_str(&o2));
    assert_eq!(stdout_str(&o2), "2 False False 1 None None None False\n");

    // PYTHONOPTIMIZE=2 matches -OO.
    let env_o2 = run_cli_with_env(None, &["-c", script], &[("PYTHONOPTIMIZE", "2")]);
    assert!(
        env_o2.status.success(),
        "PYTHONOPTIMIZE=2 failed:\n{}",
        stderr_str(&env_o2)
    );
    assert_eq!(
        stdout_str(&env_o2),
        "2 False False 1 None None None False\n"
    );
}

#[test]
fn flags_b_e_i_p_s_site_u_d_and_env_isolation() {
    let dir = TempDir::new("flags_isolation");
    dir.write("sibling_mod.py", "VAL = 99\n");
    let script = dir.write(
        "check.py",
        "import builtins, sys\n\
         print(\n\
             sys.flags.dont_write_bytecode,\n\
             sys.dont_write_bytecode,\n\
             sys.flags.ignore_environment,\n\
             sys.flags.isolated,\n\
             sys.flags.no_user_site,\n\
             sys.flags.no_site,\n\
             sys.flags.safe_path,\n\
             sys.flags.debug,\n\
             'site' in sys.modules,\n\
             hasattr(builtins, 'quit'),\n\
         )\n",
    );

    // Default run: site loaded, interactive builtins installed, sibling importable.
    let normal = run_cli_with_env(Some(dir.path()), &[script.to_str().unwrap()], &[]);
    assert!(normal.status.success(), "{}", stderr_str(&normal));
    assert_eq!(stdout_str(&normal), "0 False 0 0 0 0 False 0 True True\n");

    // -B -s -S -d -u (clustered as -BsSdu)
    let clustered = run_cli_with_env(Some(dir.path()), &["-BsSdu", script.to_str().unwrap()], &[]);
    assert!(clustered.status.success(), "{}", stderr_str(&clustered));
    assert_eq!(
        stdout_str(&clustered),
        "1 True 0 0 1 1 False 1 False False\n"
    );

    // -E ignores PYTHON* env vars.
    let ignore_env = run_cli_with_env(
        None,
        &[
            "-E",
            "-c",
            "import sys; print(sys.flags.ignore_environment, sys.flags.optimize, sys.flags.safe_path, sys.dont_write_bytecode, sys.warnoptions)",
        ],
        &[
            ("PYTHONOPTIMIZE", "2"),
            ("PYTHONSAFEPATH", "1"),
            ("PYTHONDONTWRITEBYTECODE", "1"),
            ("PYTHONWARNINGS", "error"),
        ],
    );
    assert!(ignore_env.status.success(), "{}", stderr_str(&ignore_env));
    assert_eq!(stdout_str(&ignore_env), "1 0 False False []\n");

    // -P and PYTHONSAFEPATH prevent prepending script dir / cwd to sys.path.
    let safe_p = run_cli_with_env(
        Some(dir.path()),
        &["-P", "-c", "import sys, sibling_mod"],
        &[],
    );
    assert_eq!(safe_p.status.code(), Some(1));
    assert!(stderr_str(&safe_p).contains("ModuleNotFoundError"));

    let safe_env = run_cli_with_env(
        Some(dir.path()),
        &[
            "-c",
            "import sys; print(sys.flags.safe_path, '' in sys.path)",
        ],
        &[("PYTHONSAFEPATH", "1")],
    );
    assert!(safe_env.status.success(), "{}", stderr_str(&safe_env));
    assert_eq!(stdout_str(&safe_env), "True False\n");

    // -I implies -E, -s, and -P.
    let isolated = run_cli_with_env(
        Some(dir.path()),
        &[
            "-I",
            "-c",
            "import site, sys; print(sys.flags.isolated, sys.flags.ignore_environment, sys.flags.no_user_site, sys.flags.safe_path, sys.flags.optimize, site.ENABLE_USER_SITE, '' in sys.path)",
        ],
        &[("PYTHONOPTIMIZE", "2")],
    );
    assert!(isolated.status.success(), "{}", stderr_str(&isolated));
    assert_eq!(stdout_str(&isolated), "1 1 1 True 0 False False\n");

    // PYTHONPATH allows importing from external directory (ignored with -E).
    let py_path_ok = run_cli_with_env(
        None,
        &["-c", "import sibling_mod; print(sibling_mod.VAL)"],
        &[("PYTHONPATH", dir.path().to_str().unwrap())],
    );
    assert!(py_path_ok.status.success(), "{}", stderr_str(&py_path_ok));
    assert_eq!(stdout_str(&py_path_ok), "99\n");

    let py_path_ignored = run_cli_with_env(
        None,
        &["-E", "-c", "import sibling_mod"],
        &[("PYTHONPATH", dir.path().to_str().unwrap())],
    );
    assert_eq!(py_path_ignored.status.code(), Some(1));
    assert!(stderr_str(&py_path_ignored).contains("ModuleNotFoundError"));
}

#[test]
fn warnings_flags_w_and_pythonwarnings() {
    // -W error turns warnings.warn into an exception.
    let w_err = run_cli_with_env(
        None,
        &[
            "-W",
            "error",
            "-c",
            "import sys, warnings; print(sys.warnoptions); warnings.warn('boom')",
        ],
        &[],
    );
    assert_eq!(w_err.status.code(), Some(1));
    assert_eq!(stdout_str(&w_err), "['error']\n");
    assert!(
        stderr_str(&w_err).contains("UserWarning: boom"),
        "expected UserWarning raised:\n{}",
        stderr_str(&w_err)
    );

    // Attached -Wignore suppresses warnings.
    let w_ign = run_cli_with_env(
        None,
        &[
            "-Wignore",
            "-c",
            "import warnings; warnings.warn('silenced'); print('ok')",
        ],
        &[],
    );
    assert!(w_ign.status.success(), "{}", stderr_str(&w_ign));
    assert_eq!(stdout_str(&w_ign), "ok\n");
    assert!(stderr_str(&w_ign).is_empty());

    // PYTHONWARNINGS comma-separated list.
    let env_w = run_cli_with_env(
        None,
        &[
            "-c",
            concat!(
                "import sys, warnings\n",
                "print(sys.warnoptions)\n",
                "warnings.warn('dep', DeprecationWarning)\n",
                "try:\n",
                "    warnings.warn('usr', UserWarning)\n",
                "except UserWarning as e:\n",
                "    print('caught', e)\n",
            ),
        ],
        &[("PYTHONWARNINGS", "error,ignore::DeprecationWarning")],
    );
    assert!(env_w.status.success(), "{}", stderr_str(&env_w));
    assert_eq!(
        stdout_str(&env_w),
        "['error', 'ignore::DeprecationWarning']\ncaught usr\n"
    );
}

#[test]
fn xoptions_dev_int_max_str_digits_pycache_utf8() {
    let out = run_cli_with_env(
        None,
        &[
            "-X",
            "dev",
            "-X",
            "foo=bar",
            "-X",
            "flag",
            "-X",
            "int_max_str_digits=5000",
            "-X",
            "pycache_prefix=/tmp/my_pyc",
            "-X",
            "utf8=1",
            "-X",
            "warn_default_encoding",
            "-c",
            concat!(
                "import sys\n",
                "print(sys.flags.dev_mode, sys.flags.utf8_mode, sys.flags.warn_default_encoding)\n",
                "print(sys.flags.int_max_str_digits, sys.get_int_max_str_digits())\n",
                "print(sys.pycache_prefix)\n",
                "print(sys._xoptions['dev'], sys._xoptions['foo'], sys._xoptions['flag'])\n",
                "print(len(str(10 ** 4900)) > 4300)\n",
                "try:\n",
                "    int('1' * 5001)\n",
                "    print('no_err')\n",
                "except ValueError:\n",
                "    print('limit_enforced')\n",
            ),
        ],
        &[],
    );
    assert!(out.status.success(), "{}", stderr_str(&out));
    assert_eq!(
        stdout_str(&out),
        "True 1 1\n\
         5000 5000\n\
         /tmp/my_pyc\n\
         True bar True\n\
         True\n\
         limit_enforced\n"
    );

    // -X int_max_str_digits=0 disables the limit.
    let unlimited = run_cli_with_env(
        None,
        &[
            "-X",
            "int_max_str_digits=0",
            "-c",
            "import sys; print(sys.flags.int_max_str_digits, sys.get_int_max_str_digits(), len(str(int('9' * 6000))))",
        ],
        &[],
    );
    assert!(unlimited.status.success(), "{}", stderr_str(&unlimited));
    assert_eq!(stdout_str(&unlimited), "0 0 6000\n");

    // Invalid int_max_str_digits fails startup cleanly.
    let bad_limit = run_cli_with_env(None, &["-X", "int_max_str_digits=100", "-c", "pass"], &[]);
    assert!(!bad_limit.status.success());
    assert!(stderr_str(&bad_limit).contains("int_max_str_digits"));
}

#[test]
fn flags_bytes_warning_verbose_skip_first_line_quiet_inspect_help_and_pyc() {
    // -bb raises BytesWarning on str(b'abc').
    let bb = run_cli_with_env(
        None,
        &[
            "-bb",
            "-c",
            concat!(
                "import sys\n",
                "print(sys.flags.bytes_warning)\n",
                "try:\n",
                "    str(b'abc')\n",
                "except BytesWarning:\n",
                "    print('raised_bytes_warning')\n",
            ),
        ],
        &[],
    );
    assert!(bb.status.success(), "{}", stderr_str(&bb));
    assert_eq!(stdout_str(&bb), "2\nraised_bytes_warning\n");

    // -v traces imports to stderr.
    let verbose = run_cli_with_env(
        None,
        &[
            "-v",
            "-c",
            "import math; print(sys.flags.verbose if 'sys' in globals() else 1)",
        ],
        &[],
    );
    assert!(verbose.status.success(), "{}", stderr_str(&verbose));
    assert!(
        stderr_str(&verbose).contains("import 'math'"),
        "expected verbose import trace:\n{}",
        stderr_str(&verbose)
    );

    // -x skips the first line of a script file while keeping line numbers.
    let dir = TempDir::new("skip_first_line");
    let script = dir.write(
        "weird_header.py",
        "NOT VALID PYTHON HEADER LINE @@@\nimport sys\nprint('line', sys._getframe().f_lineno)\n",
    );
    let skip_out = run_cli_with_env(None, &["-x", script.to_str().unwrap()], &[]);
    assert!(skip_out.status.success(), "{}", stderr_str(&skip_out));
    assert_eq!(stdout_str(&skip_out), "line 3\n");

    // -q suppresses REPL banner.
    let repl_quiet = run_cli_with_stdin(None, &["-q"], "print(1 + 2)\nexit\n");
    assert!(repl_quiet.status.success(), "{}", stderr_str(&repl_quiet));
    assert!(!stdout_str(&repl_quiet).contains("PyRust 0.2"));
    assert!(stdout_str(&repl_quiet).contains("3"));

    // -i drops into interactive REPL preserving script variables.
    let inspect_out = run_cli_with_stdin(
        None,
        &["-i", "-c", "answer = 41"],
        "print(answer + 1)\nexit\n",
    );
    assert!(inspect_out.status.success(), "{}", stderr_str(&inspect_out));
    assert!(stdout_str(&inspect_out).contains("42"));

    // --check-hash-based-pycs accepts default|always|never and rejects invalid.
    for mode in ["default", "always", "never"] {
        let ok = run_cli_with_env(
            None,
            &[
                &format!("--check-hash-based-pycs={mode}"),
                "-c",
                "print('ok')",
            ],
            &[],
        );
        assert!(
            ok.status.success(),
            "mode {mode} failed: {}",
            stderr_str(&ok)
        );
        assert_eq!(stdout_str(&ok), "ok\n");
    }
    let bad_pyc = run_cli_with_env(None, &["--check-hash-based-pycs=bogus"], &[]);
    assert_eq!(bad_pyc.status.code(), Some(2));

    // --help-env, --help-xoptions, --help-all exit 0 with expected sections.
    let help_env = run_cli_with_env(None, &["--help-env"], &[]);
    assert_eq!(help_env.status.code(), Some(0));
    assert!(stdout_str(&help_env).contains("PYTHONOPTIMIZE"));
    assert!(stdout_str(&help_env).contains("PYTHONWARNINGS"));

    let help_x = run_cli_with_env(None, &["--help-xoptions"], &[]);
    assert_eq!(help_x.status.code(), Some(0));
    assert!(stdout_str(&help_x).contains("-X dev"));
    assert!(stdout_str(&help_x).contains("-X int_max_str_digits=N"));

    let help_all = run_cli_with_env(None, &["--help-all"], &[]);
    assert_eq!(help_all.status.code(), Some(0));
    let all_out = stdout_str(&help_all);
    assert!(
        all_out.contains("-c cmd")
            && all_out.contains("PYTHONOPTIMIZE")
            && all_out.contains("-X dev")
    );

    // Interactive builtins installed by default: quit(7) exits with code 7.
    let quit_code = run_cli_with_env(None, &["-c", "quit(7)"], &[]);
    assert_eq!(quit_code.status.code(), Some(7));

    // -m site prints site info and exits 0.
    let m_site = run_cli_with_env(None, &["-m", "site"], &[]);
    assert!(m_site.status.success(), "{}", stderr_str(&m_site));
    let m_site_out = stdout_str(&m_site);
    assert!(m_site_out.contains("sys.path = [") && m_site_out.contains("USER_BASE:"));

    // -m pydoc prints usage with no args, documents topics/objects, and writes HTML with -w.
    let m_pydoc_usage = run_cli_with_env(None, &["-m", "pydoc"], &[]);
    assert!(
        m_pydoc_usage.status.success(),
        "{}",
        stderr_str(&m_pydoc_usage)
    );
    assert!(stdout_str(&m_pydoc_usage).contains("pydoc - the Python documentation tool"));

    let m_pydoc_topic = run_cli_with_env(None, &["-m", "pydoc", "ASSERTION"], &[]);
    assert!(
        m_pydoc_topic.status.success(),
        "{}",
        stderr_str(&m_pydoc_topic)
    );
    assert!(stdout_str(&m_pydoc_topic).contains("assert"));

    let m_pydoc_obj = run_cli_with_env(None, &["-m", "pydoc", "math.sin"], &[]);
    assert!(m_pydoc_obj.status.success(), "{}", stderr_str(&m_pydoc_obj));
    assert!(stdout_str(&m_pydoc_obj).contains("sin"));

    let m_pydoc_missing = run_cli_with_env(None, &["-m", "pydoc", "no_such_module_3255"], &[]);
    assert_eq!(m_pydoc_missing.status.code(), Some(1));
    assert!(stdout_str(&m_pydoc_missing).contains("No Python documentation found"));
}
