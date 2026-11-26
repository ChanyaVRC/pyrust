"""CPython 3.12 parity tests for `venv` and `ensurepip` (issue #3267)."""

import contextlib
import io
import os
import sys
import tempfile
import types
import venv
import ensurepip
import ensurepip.__main__
import ensurepip._uninstall


# ── 1. venv module surface & EnvBuilder defaults ─────────────────────────────

print("venv_core_deps:", venv.CORE_VENV_DEPS)
print("venv_has_logger:", hasattr(venv, "logger"))
print("venv_pkg:", venv.__package__, hasattr(venv, "__path__"))

b_default = venv.EnvBuilder()
print(
    "builder_defaults:",
    b_default.system_site_packages,
    b_default.clear,
    b_default.symlinks,
    b_default.upgrade,
    b_default.with_pip,
    b_default.orig_prompt,
    b_default.prompt,
    b_default.upgrade_deps,
)

b_dot = venv.EnvBuilder(prompt=".")
print(
    "builder_dot_prompt:",
    b_dot.orig_prompt,
    b_dot.prompt == os.path.basename(os.getcwd()),
)


# ── 2. EnvBuilder.ensure_directories, replace_variables, create_configuration ─

with tempfile.TemporaryDirectory() as _tmp:
    tmp = os.path.realpath(_tmp)
    # Reject path containing os.pathsep
    bad_dir = os.path.join(tmp, f"bad{os.pathsep}dir")
    try:
        venv.EnvBuilder(with_pip=False).ensure_directories(bad_dir)
        print("bad_pathsep: unexpected_ok")
    except ValueError as e:
        print("bad_pathsep:", "PATH separator" in str(e))

    # Reject existing regular file
    file_path = os.path.join(tmp, "not_a_dir")
    with open(file_path, "w", encoding="utf-8") as f:
        f.write("x")
    try:
        venv.EnvBuilder(with_pip=False).ensure_directories(file_path)
        print("existing_file: unexpected_ok")
    except ValueError as e:
        print("existing_file:", "Unable to create directory" in str(e))

    # Full venv creation with prompt and without pip
    env1 = os.path.join(tmp, "my_env")
    builder = venv.EnvBuilder(with_pip=False, prompt="demo-env")
    builder.create(env1)

    expected_bin = "Scripts" if os.name == "nt" else "bin"
    bin_dir = os.path.join(env1, expected_bin)
    if os.name == "nt":
        site_pkgs = os.path.join(env1, "Lib", "site-packages")
        inc_dir = os.path.join(env1, "Include")
    else:
        site_pkgs = os.path.join(
            env1,
            "lib",
            f"python{sys.version_info[0]}.{sys.version_info[1]}",
            "site-packages",
        )
        inc_dir = os.path.join(
            env1,
            "include",
            f"python{sys.version_info[0]}.{sys.version_info[1]}",
        )

    print(
        "env1_dirs:",
        os.path.isdir(bin_dir),
        os.path.isdir(site_pkgs),
        os.path.isdir(inc_dir),
    )

    cfg_path = os.path.join(env1, "pyvenv.cfg")
    print("env1_cfg_exists:", os.path.isfile(cfg_path))
    with open(cfg_path, encoding="utf-8") as f:
        cfg_lines = {}
        for line in f:
            if "=" in line:
                k, v = line.split("=", 1)
                cfg_lines[k.strip()] = v.strip()

    print(
        "env1_cfg_keys:",
        "home" in cfg_lines,
        cfg_lines.get("include-system-site-packages"),
        cfg_lines.get("version", "").startswith("3.12."),
        cfg_lines.get("prompt"),
        "executable" in cfg_lines,
        "-m venv" in cfg_lines.get("command", ""),
        "--without-pip" in cfg_lines.get("command", ""),
    )

    # Activation scripts exist and have placeholders substituted
    activate_sh = os.path.join(bin_dir, "activate")
    activate_ps1 = os.path.join(bin_dir, "Activate.ps1")
    print("activate_common:", os.path.isfile(activate_sh), os.path.isfile(activate_ps1))
    with open(activate_sh, encoding="utf-8") as f:
        act_text = f.read()
    print(
        "activate_substituted:",
        "__VENV_DIR__" not in act_text,
        "__VENV_BIN_NAME__" not in act_text,
        "__VENV_PROMPT__" not in act_text,
        "(demo-env) " in act_text,
        os.path.abspath(env1) in act_text,
        f"/{expected_bin}:" in act_text,
    )

    if os.name == "nt":
        act_bat = os.path.join(bin_dir, "activate.bat")
        deact_bat = os.path.join(bin_dir, "deactivate.bat")
        print("activate_plat:", os.path.isfile(act_bat), os.path.isfile(deact_bat))
        with open(act_bat, encoding="utf-8") as f:
            bat_text = f.read()
        print("bat_substituted:", "(demo-env) " in bat_text, "__VENV_DIR__" not in bat_text)
    else:
        act_csh = os.path.join(bin_dir, "activate.csh")
        act_fish = os.path.join(bin_dir, "activate.fish")
        print("activate_plat:", os.path.isfile(act_csh), os.path.isfile(act_fish))
        with open(act_csh, encoding="utf-8") as f:
            csh_text = f.read()
        print("csh_substituted:", "(demo-env) " in csh_text, "__VENV_DIR__" not in csh_text)

    # Clear existing files when clear=True
    marker = os.path.join(env1, "stale.txt")
    with open(marker, "w", encoding="utf-8") as f:
        f.write("old")
    venv.EnvBuilder(clear=True, with_pip=False).create(env1)
    print("cleared_stale:", not os.path.exists(marker), os.path.isfile(cfg_path))

    # system_site_packages=True writes include-system-site-packages = true
    env_sys = os.path.join(tmp, "sys_site_env")
    venv.create(env_sys, system_site_packages=True, with_pip=False)
    with open(os.path.join(env_sys, "pyvenv.cfg"), encoding="utf-8") as f:
        sys_cfg = f.read()
    print("sys_site_true:", "include-system-site-packages = true" in sys_cfg)


# ── 3. EnvBuilder subclassing, context SimpleNamespace, install_scripts ──────

with tempfile.TemporaryDirectory() as _tmp:
    tmp = os.path.realpath(_tmp)
    events = []

    class CustomBuilder(venv.EnvBuilder):
        def replace_variables(self, text, context):
            text = super().replace_variables(text, context)
            return text.replace("__CUSTOM__", "OK_42")

        def post_setup(self, context):
            events.append(
                (
                    isinstance(context, types.SimpleNamespace),
                    context.env_name,
                    context.prompt,
                    context.bin_name,
                    os.path.isfile(context.cfg_path),
                    os.path.exists(context.env_exe),
                )
            )

    env_sub = os.path.join(tmp, "sub_env")
    cb = CustomBuilder(with_pip=False, prompt="custom")
    cb.create(env_sub)
    print("subclass_post_setup:", events)

    # Custom install_scripts
    scripts_src = os.path.join(tmp, "my_scripts")
    os.makedirs(os.path.join(scripts_src, "common"))
    os.makedirs(os.path.join(scripts_src, "ignored_os"))
    with open(os.path.join(scripts_src, "common", "hello.txt"), "w", encoding="utf-8") as f:
        f.write("dir=__VENV_NAME__ custom=__CUSTOM__ prompt=__VENV_PROMPT__")
    with open(os.path.join(scripts_src, "ignored_os", "skip.txt"), "w", encoding="utf-8") as f:
        f.write("should not be copied")

    ctx = cb.ensure_directories(env_sub)
    cb.install_scripts(ctx, scripts_src)
    with open(os.path.join(ctx.bin_path, "hello.txt"), encoding="utf-8") as f:
        print("install_scripts_text:", f.read())
    print("install_scripts_skip:", os.path.exists(os.path.join(ctx.bin_path, "skip.txt")))


# ── 4. venv.main() CLI parsing & multiple ENV_DIRs ───────────────────────────

with tempfile.TemporaryDirectory() as _tmp:
    tmp = os.path.realpath(_tmp)
    d1 = os.path.join(tmp, "cli_a")
    d2 = os.path.join(tmp, "cli_b")
    venv.main(["--without-pip", "--prompt", "multi", d1, d2])
    print(
        "main_multi:",
        os.path.isfile(os.path.join(d1, "pyvenv.cfg")),
        os.path.isfile(os.path.join(d2, "pyvenv.cfg")),
    )

    try:
        venv.main(["--clear", "--upgrade", d1])
        print("main_conflict: unexpected_ok")
    except ValueError as e:
        print("main_conflict:", "--upgrade and --clear" in str(e))


# ── 5. ensurepip module surface, version, helpers & CLI ──────────────────────

print("ensurepip_all:", ensurepip.__all__)
print("ensurepip_pkg:", ensurepip.__package__, hasattr(ensurepip, "__path__"))
ver = ensurepip.version()
ver_parts = ver.split(".")
print(
    "ensurepip_version_valid:",
    isinstance(ver, str),
    len(ver_parts) >= 2 and all(p.isdigit() for p in ver_parts),
)
print("ensurepip_pkg_names:", ensurepip._PACKAGE_NAMES)
pkgs = ensurepip._get_packages()
print("ensurepip_get_packages:", "pip" in pkgs, pkgs["pip"].version == ver)

with tempfile.TemporaryDirectory() as _tmp:
    tmp = os.path.realpath(_tmp)
    with open(os.path.join(tmp, "pip-9.8.7-py3-none-any.whl"), "wb") as f:
        f.write(b"")
    with open(os.path.join(tmp, "other-1.0-py3-none-any.whl"), "wb") as f:
        f.write(b"")
    found = ensurepip._find_packages(tmp)
    print(
        "ensurepip_find_packages:",
        list(found.keys()),
        found["pip"].version,
        found["pip"].wheel_name,
        os.path.basename(found["pip"].wheel_path),
    )

# _disable_pip_configuration_settings strips PIP_* and sets PIP_CONFIG_FILE
saved_pip = {k: v for k, v in os.environ.items() if k.startswith("PIP_")}
try:
    os.environ["PIP_TEST_DUMMY"] = "1"
    os.environ["PIP_INDEX_URL"] = "https://example.invalid"
    ensurepip._disable_pip_configuration_settings()
    print(
        "disable_pip_cfg:",
        "PIP_TEST_DUMMY" not in os.environ,
        "PIP_INDEX_URL" not in os.environ,
        os.environ.get("PIP_CONFIG_FILE") == os.devnull,
    )
finally:
    for k in [k for k in os.environ.keys() if k.startswith("PIP_")]:
        del os.environ[k]
    for k, v in saved_pip.items():
        os.environ[k] = v

# bootstrap rejects positional args and rejects altinstall + default_pip together
try:
    ensurepip.bootstrap("positional")
    print("bootstrap_pos: unexpected_ok")
except TypeError:
    print("bootstrap_pos: TypeError")

try:
    ensurepip.bootstrap(altinstall=True, default_pip=True)
    print("bootstrap_conflict: unexpected_ok")
except ValueError as e:
    print("bootstrap_conflict:", "altinstall and default_pip" in str(e))

# CLI --version for ensurepip and ensurepip._uninstall
buf = io.StringIO()
ep_code = None
with contextlib.redirect_stdout(buf):
    try:
        ensurepip._main(["--version"])
    except SystemExit as e:
        ep_code = e.code
print("ensurepip_cli_code:", ep_code, buf.getvalue().strip() == f"pip {ver}")

buf2 = io.StringIO()
un_code = None
with contextlib.redirect_stdout(buf2):
    try:
        ensurepip._uninstall._main(["--version"])
    except SystemExit as e:
        un_code = e.code
print("uninstall_cli_code:", un_code, buf2.getvalue().strip() == f"pip {ver}")
