//! End-to-end integration tests for `venv`, `pyvenv.cfg` startup detection,
//! `site` virtual-environment handling, and `ensurepip` CLI (issue #3267).

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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
        path.push(format!("pyrust_venv_{tag}_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&path).expect("create temporary test directory");
        Self(path)
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

fn run_bin(bin: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(bin);
    for key in [
        "PYTHONHOME",
        "PYTHONPATH",
        "PYTHONNOUSERSITE",
        "PYTHONSAFEPATH",
        "VIRTUAL_ENV",
    ] {
        cmd.env_remove(key);
    }
    cmd.args(args).output().expect("spawn executable")
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n")
}

fn venv_python_path(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

fn venv_site_packages_path(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Lib").join("site-packages")
    } else {
        venv_dir
            .join("lib")
            .join("python3.12")
            .join("site-packages")
    }
}

fn norm_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[test]
fn venv_cli_creates_working_env_with_pyvenv_cfg_detection_and_site_packages() {
    let tmp = TempDir::new("basic");
    let venv_dir = tmp.path().join("myenv");
    let venv_str = venv_dir.to_str().expect("utf-8 temp path");

    // 1. Create virtual environment via `pyrust -m venv --without-pip <dir>`.
    let create_out = run_bin(&pyrust_bin(), &["-m", "venv", "--without-pip", venv_str]);
    assert!(
        create_out.status.success(),
        "pyrust -m venv failed:\nstdout: {}\nstderr: {}",
        stdout_str(&create_out),
        stderr_str(&create_out)
    );

    let venv_py = venv_python_path(&venv_dir);
    let site_pkgs = venv_site_packages_path(&venv_dir);
    assert!(
        venv_py.exists(),
        "expected venv python executable at {}",
        venv_py.display()
    );
    assert!(
        site_pkgs.is_dir(),
        "expected venv site-packages dir at {}",
        site_pkgs.display()
    );
    assert!(venv_dir.join("pyvenv.cfg").is_file());

    // 2. Install a module and a .pth extra directory inside the venv's site-packages.
    std::fs::write(
        site_pkgs.join("my_venv_pkg.py"),
        "MARKER = 'from-venv-site-packages'\n",
    )
    .expect("write venv package");
    let extra_dir = tmp.path().join("pth_extra");
    std::fs::create_dir_all(&extra_dir).expect("create pth extra dir");
    std::fs::write(extra_dir.join("pth_only_mod.py"), "PTH_VAL = 12345\n")
        .expect("write pth module");
    std::fs::write(
        site_pkgs.join("custom_extra.pth"),
        format!("{}\n", extra_dir.to_str().unwrap()),
    )
    .expect("write .pth file");

    // 3. Execute the venv's python binary and verify sys.prefix, sys.base_prefix,
    //    site.PREFIXES, site.ENABLE_USER_SITE, sys.path, and package imports.
    let probe = r#"
import os, site, sys, my_venv_pkg, pth_only_mod
print(sys.prefix)
print(sys.exec_prefix)
print(sys.base_prefix)
print(sys.base_exec_prefix)
print(sys.prefix != sys.base_prefix)
print(sys.prefix == sys.exec_prefix)
print(sys.base_prefix == sys.base_exec_prefix)
print(site.ENABLE_USER_SITE)
print(len(site.PREFIXES), site.PREFIXES[0] == sys.prefix)
print(my_venv_pkg.MARKER, pth_only_mod.PTH_VAL)
"#;
    let run_out = run_bin(&venv_py, &["-c", probe]);
    assert!(
        run_out.status.success(),
        "venv python execution failed:\nstdout: {}\nstderr: {}",
        stdout_str(&run_out),
        stderr_str(&run_out)
    );

    let run_stdout = stdout_str(&run_out);
    let lines: Vec<&str> = run_stdout.lines().collect();
    assert_eq!(lines.len(), 10, "unexpected probe output:\n{run_stdout}");
    assert_eq!(norm_path(Path::new(lines[0])), norm_path(&venv_dir));
    assert_eq!(norm_path(Path::new(lines[1])), norm_path(&venv_dir));
    assert_ne!(
        norm_path(Path::new(lines[0])),
        norm_path(Path::new(lines[2]))
    );
    assert_eq!(lines[4], "True");
    assert_eq!(lines[5], "True");
    assert_eq!(lines[6], "True");
    assert_eq!(lines[7], "False");
    assert_eq!(lines[8], "1 True");
    assert_eq!(lines[9], "from-venv-site-packages 12345");

    // 4. Base pyrust binary must NOT see the venv's private package.
    let base_out = run_bin(&pyrust_bin(), &["-c", "import my_venv_pkg"]);
    assert!(!base_out.status.success());
    assert!(stderr_str(&base_out).contains("ModuleNotFoundError"));

    // 5. Running the venv binary with `-S` still sets sys.prefix != sys.base_prefix
    //    via startup pyvenv.cfg detection while skipping `import site`.
    let no_site_out = run_bin(
        &venv_py,
        &[
            "-S",
            "-c",
            "import sys; print(sys.prefix); print(sys.prefix != sys.base_prefix); print('site' in sys.modules)",
        ],
    );
    assert!(
        no_site_out.status.success(),
        "venv python -S failed:\n{}",
        stderr_str(&no_site_out)
    );
    let ns_stdout = stdout_str(&no_site_out);
    let ns_lines: Vec<&str> = ns_stdout.lines().collect();
    assert_eq!(ns_lines.len(), 3);
    assert_eq!(norm_path(Path::new(ns_lines[0])), norm_path(&venv_dir));
    assert_eq!(ns_lines[1], "True");
    assert_eq!(ns_lines[2], "False");
}

#[test]
fn venv_system_site_packages_true_keeps_base_prefix_in_site_prefixes() {
    let tmp = TempDir::new("sys_site");
    let venv_dir = tmp.path().join("sys_env");
    let venv_str = venv_dir.to_str().expect("utf-8 temp path");

    let create_out = run_bin(
        &pyrust_bin(),
        &[
            "-m",
            "venv",
            "--without-pip",
            "--system-site-packages",
            venv_str,
        ],
    );
    assert!(
        create_out.status.success(),
        "venv --system-site-packages failed:\n{}",
        stderr_str(&create_out)
    );

    let venv_py = venv_python_path(&venv_dir);
    let out = run_bin(
        &venv_py,
        &[
            "-c",
            "import site, sys\n\
             print(sys.prefix != sys.base_prefix)\n\
             print(sys.prefix in site.PREFIXES)\n\
             print(sys.base_prefix in site.PREFIXES)\n\
             print(site.ENABLE_USER_SITE)\n",
        ],
    );
    assert!(
        out.status.success(),
        "venv python failed:\n{}",
        stderr_str(&out)
    );
    assert_eq!(stdout_str(&out), "True\nTrue\nTrue\nTrue\n");
}

#[cfg(unix)]
#[test]
fn venv_copies_and_symlinks_both_detect_pyvenv_cfg_on_unix() {
    let tmp = TempDir::new("unix_modes");
    for (name, flag) in [("sym_env", "--symlinks"), ("copy_env", "--copies")] {
        let venv_dir = tmp.path().join(name);
        let venv_str = venv_dir.to_str().expect("utf-8 path");
        let create_out = run_bin(
            &pyrust_bin(),
            &["-m", "venv", "--without-pip", flag, venv_str],
        );
        assert!(
            create_out.status.success(),
            "venv {flag} failed:\n{}",
            stderr_str(&create_out)
        );
        let venv_py = venv_python_path(&venv_dir);
        let out = run_bin(
            &venv_py,
            &[
                "-c",
                "import sys; print(sys.prefix); print(sys.prefix != sys.base_prefix)",
            ],
        );
        assert!(
            out.status.success(),
            "venv {flag} python failed:\n{}",
            stderr_str(&out)
        );
        let out_stdout = stdout_str(&out);
        let lines: Vec<&str> = out_stdout.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(norm_path(Path::new(lines[0])), norm_path(&venv_dir));
        assert_eq!(lines[1], "True");
    }
}

#[test]
fn venv_default_with_pip_and_ensurepip_cli_are_offline_safe() {
    let tmp = TempDir::new("ensurepip_cli");
    let venv_dir = tmp.path().join("with_pip_env");
    let venv_str = venv_dir.to_str().expect("utf-8 path");

    // Default `pyrust -m venv <dir>` invokes `-m ensurepip --upgrade --default-pip`
    // inside the created venv; must succeed cleanly offline.
    let create_out = run_bin(&pyrust_bin(), &["-m", "venv", venv_str]);
    assert!(
        create_out.status.success(),
        "default pyrust -m venv failed:\nstdout: {}\nstderr: {}",
        stdout_str(&create_out),
        stderr_str(&create_out)
    );
    assert!(venv_dir.join("pyvenv.cfg").is_file());

    // `pyrust -m ensurepip --version` and `pyrust -m ensurepip._uninstall --version`
    let ep_ver = run_bin(&pyrust_bin(), &["-m", "ensurepip", "--version"]);
    assert!(ep_ver.status.success(), "{}", stderr_str(&ep_ver));
    assert_eq!(stdout_str(&ep_ver), "pip 24.0\n");

    let ep_main_ver = run_bin(&pyrust_bin(), &["-m", "ensurepip.__main__", "--version"]);
    assert!(ep_main_ver.status.success(), "{}", stderr_str(&ep_main_ver));
    assert_eq!(stdout_str(&ep_main_ver), "pip 24.0\n");

    let un_ver = run_bin(&pyrust_bin(), &["-m", "ensurepip._uninstall", "--version"]);
    assert!(un_ver.status.success(), "{}", stderr_str(&un_ver));
    assert_eq!(stdout_str(&un_ver), "pip 24.0\n");

    // Invalid mutually incompatible flags raise ValueError and exit non-zero.
    let conflict = run_bin(
        &pyrust_bin(),
        &["-m", "ensurepip", "--altinstall", "--default-pip"],
    );
    assert!(!conflict.status.success());
    assert!(stderr_str(&conflict).contains("ValueError"));
}
