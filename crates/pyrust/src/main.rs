mod ast;
mod builtin_modules;
mod builtin_registry;
mod bytecode;
mod compiler;
mod error;
mod interpreter;
mod lexer;
mod mir;
mod optimizer;
mod parser;
mod token;
mod value;

use std::io::Read;

use error::{PyError, Result};
use interpreter::{Interpreter, RuntimeConfig, lex_parse_to_exc};
use lexer::Lexer;
use parser::Parser;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

const PYRUST_VERSION_SHORT: &str = "Python 3.12.0 (PyRust 0.2)";

#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    Repl,
    PrintVersion { verbose: bool },
    PrintHelp,
    PrintHelpEnv,
    PrintHelpXOptions,
    PrintHelpAll,
    EmitMir { path: String },
    RunFile { path: String, args: Vec<String> },
    RunCommand { command: String, args: Vec<String> },
    RunModule { module: String, args: Vec<String> },
    RunStdin { args: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpMode {
    Main,
    Env,
    XOptions,
    All,
}

#[derive(Debug)]
struct ParsedCli {
    action: CliAction,
    config: RuntimeConfig,
}

#[derive(Debug, PartialEq, Eq)]
struct CliUsageError {
    message: String,
    exit_code: i32,
}

impl CliUsageError {
    fn usage_line(prog: &str) -> String {
        format!(
            "usage: {prog} [option] ... [-c cmd | -m mod | file | -] [arg] ...\n\
             Try `{prog} -h' for more information."
        )
    }

    fn missing_arg(flag: &str, prog: &str) -> Self {
        Self {
            message: format!(
                "Argument expected for the {flag} option\n{}",
                Self::usage_line(prog)
            ),
            exit_code: 2,
        }
    }

    fn unknown_option(opt: &str, prog: &str) -> Self {
        Self {
            message: format!("Unknown option: {opt}\n{}", Self::usage_line(prog)),
            exit_code: 2,
        }
    }

    fn fatal(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit_code: 1,
        }
    }
}

fn format_version(verbose: bool) -> String {
    if verbose {
        format!(
            "{PYRUST_VERSION_SHORT} [{}/{}]",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    } else {
        PYRUST_VERSION_SHORT.to_string()
    }
}

fn format_help(prog: &str) -> String {
    format!(
        "usage: {prog} [option] ... [-c cmd | -m mod | file | -] [arg] ...\n\
Options (and corresponding environment variables):\n\
-b     : issue warnings about converting bytes/bytearray to str and comparing\n\
         bytes/bytearray with str or bytes with int. (-bb: issue errors)\n\
-B     : don't write .pyc files on import; also PYTHONDONTWRITEBYTECODE=x\n\
-c cmd : program passed in as string (terminates option list)\n\
-d     : turn on parser debugging output; also PYTHONDEBUG=x\n\
-E     : ignore PYTHON* environment variables (such as PYTHONPATH)\n\
-h     : print this help message and exit (also -? or --help)\n\
-i     : inspect interactively after running script; also PYTHONINSPECT=x\n\
-I     : isolate Python from the user's environment (implies -E, -P and -s)\n\
-m mod : run library module as a script (terminates option list)\n\
-O     : remove assert and __debug__-dependent statements; also PYTHONOPTIMIZE=x\n\
-OO    : do -O changes and also discard docstrings\n\
-P     : don't prepend a potentially unsafe path to sys.path; also PYTHONSAFEPATH\n\
-q     : don't print version and copyright messages on interactive startup\n\
-s     : don't add user site directory to sys.path; also PYTHONNOUSERSITE=x\n\
-S     : don't imply 'import site' on initialization\n\
-u     : force the stdout and stderr streams to be unbuffered; also PYTHONUNBUFFERED=x\n\
-v     : verbose (trace import statements); also PYTHONVERBOSE=x\n\
-V     : print the Python version number and exit (also --version)\n\
         when given twice, print more information about the build\n\
-W arg : warning control; arg is action:message:category:module:lineno\n\
         also PYTHONWARNINGS=arg\n\
-x     : skip first line of source, allowing use of non-Unix forms of #!cmd\n\
-X opt : set implementation-specific option\n\
--check-hash-based-pycs always|default|never:\n\
         control how Python invalidates hash-based .pyc files\n\
--help-env: print help about Python environment variables and exit\n\
--help-xoptions: print help about implementation-specific -X options and exit\n\
--help-all: print complete help information and exit\n\
--emit mir : compile a file to SSA MIR and print it\n\
--     : terminate option list\n\
\n\
Arguments:\n\
file   : program read from script file\n\
-      : program read from stdin (default; interactive mode if a tty)\n\
arg ...: arguments passed to program in sys.argv[1:]"
    )
}

fn format_help_env() -> String {
    let sep = if cfg!(windows) { ";" } else { ":" };
    format!(
        "Environment variables that change behavior:\n\
PYTHONSTARTUP   : file executed on interactive startup (no default)\n\
PYTHONPATH      : '{sep}'-separated list of directories prefixed to the\n\
                  default module search path.  The result is sys.path.\n\
PYTHONHOME      : alternate <prefix> directory (or <prefix>{sep}<exec_prefix>).\n\
PYTHONPLATLIBDIR: override sys.platlibdir\n\
PYTHONHASHSEED  : if set to 'random', a random value seeds str/bytes hashes;\n\
                  set to 0 to disable hash randomization.\n\
PYTHONUSERBASE  : alternate user base directory for site.getuserbase()\n\
PYTHONBREAKPOINT: if set to 0, disables the default debugger.\n\
\n\
These variables have equivalent command-line options (see --help for details):\n\
PYTHONDEBUG     : enable parser debug mode (-d)\n\
PYTHONDEVMODE   : enable Python Development Mode (-X dev)\n\
PYTHONDONTWRITEBYTECODE: don't write .pyc files (-B)\n\
PYTHONINSPECT   : inspect interactively after running script (-i)\n\
PYTHONINTMAXSTRDIGITS: limit the size of int<->str conversions;\n\
                  0 disables the limit (-X int_max_str_digits=N)\n\
PYTHONNOUSERSITE: disable user site directory (-s)\n\
PYTHONOPTIMIZE  : enable level 1 optimizations (-O)\n\
PYTHONPYCACHEPREFIX: root directory for bytecode cache (pyc) files\n\
                  (-X pycache_prefix)\n\
PYTHONSAFEPATH  : don't prepend a potentially unsafe path to sys.path (-P)\n\
PYTHONUNBUFFERED: disable stdout/stderr buffering (-u)\n\
PYTHONUTF8      : control the UTF-8 mode (-X utf8)\n\
PYTHONVERBOSE   : trace import statements (-v)\n\
PYTHONWARNDEFAULTENCODING: enable opt-in EncodingWarning (-X warn_default_encoding)\n\
PYTHONWARNINGS  : warning control (-W)"
    )
}

fn format_help_xoptions() -> &'static str {
    "The following implementation-specific options are available:\n\
-X dev : enable Python Development Mode; also PYTHONDEVMODE\n\
-X faulthandler: dump the Python traceback on fatal errors\n\
-X frozen_modules=[on|off]: whether to use frozen modules\n\
-X importtime: show how long each import takes\n\
-X int_max_str_digits=N: limit the size of int<->str conversions;\n\
         0 disables the limit; also PYTHONINTMAXSTRDIGITS\n\
-X no_debug_ranges: don't include extra location information in code objects\n\
-X pycache_prefix=PATH: write .pyc files to a parallel tree instead of to the\n\
         code tree; also PYTHONPYCACHEPREFIX\n\
-X showrefcount: output total reference count on exit\n\
-X tracemalloc[=N]: trace Python memory allocations\n\
-X utf8[=0|1]: enable (1) or disable (0) UTF-8 mode; also PYTHONUTF8\n\
-X warn_default_encoding: enable opt-in EncodingWarning for 'encoding=None';\n\
         also PYTHONWARNDEFAULTENCODING"
}

fn parse_env_count(raw: &str) -> i64 {
    if raw.is_empty() {
        return 0;
    }
    match raw.trim().parse::<u32>() {
        Ok(n) => i64::from(n),
        Err(_) => 1,
    }
}

fn validate_int_max_str_digits(
    raw: &str,
    source_label: &str,
) -> std::result::Result<i64, CliUsageError> {
    let min = pyrust_core::INT_MAX_STR_DIGITS_MIN as i64;
    match raw.trim().parse::<i64>() {
        Ok(n) if n == 0 || n >= min => Ok(n),
        _ => Err(CliUsageError::fatal(format!(
            "Fatal Python error: init_sys_config: {source_label}: invalid limit; \
             must be >= {min} or 0 for unlimited."
        ))),
    }
}

fn parse_cli_args_with_env<F>(
    args: &[String],
    get_env: F,
) -> std::result::Result<ParsedCli, CliUsageError>
where
    F: Fn(&str) -> Option<String>,
{
    let prog = args.first().map(String::as_str).unwrap_or("pyrust");
    let mut config = RuntimeConfig {
        orig_argv: args.to_vec(),
        ..RuntimeConfig::default()
    };
    let mut version_count: usize = 0;
    let mut help_mode: Option<HelpMode> = None;
    let mut cli_warnoptions: Vec<String> = Vec::new();
    let mut action: Option<CliAction> = None;
    let mut i = 1;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            action = Some(match args.get(i + 1) {
                None => CliAction::Repl,
                Some(next) if next == "-" => CliAction::RunStdin {
                    args: args[i + 2..].to_vec(),
                },
                Some(path) => CliAction::RunFile {
                    path: path.clone(),
                    args: args[i + 2..].to_vec(),
                },
            });
            break;
        }
        if arg == "-" {
            action = Some(CliAction::RunStdin {
                args: args[i + 1..].to_vec(),
            });
            break;
        }
        if arg == "--version" {
            version_count += 1;
            i += 1;
            continue;
        }
        if arg == "--help" {
            help_mode = Some(HelpMode::Main);
            i += 1;
            continue;
        }
        if arg == "--help-env" {
            help_mode = Some(HelpMode::Env);
            i += 1;
            continue;
        }
        if arg == "--help-xoptions" {
            help_mode = Some(HelpMode::XOptions);
            i += 1;
            continue;
        }
        if arg == "--help-all" {
            help_mode = Some(HelpMode::All);
            i += 1;
            continue;
        }
        if arg == "--check-hash-based-pycs" {
            let Some(val) = args.get(i + 1) else {
                return Err(CliUsageError::missing_arg("--check-hash-based-pycs", prog));
            };
            if !matches!(val.as_str(), "default" | "always" | "never") {
                return Err(CliUsageError {
                    message: format!(
                        "unknown option --check-hash-based-pycs={val}\n{}",
                        CliUsageError::usage_line(prog)
                    ),
                    exit_code: 2,
                });
            }
            config.check_hash_based_pycs = val.clone();
            i += 2;
            continue;
        }
        if let Some(val) = arg.strip_prefix("--check-hash-based-pycs=") {
            if !matches!(val, "default" | "always" | "never") {
                return Err(CliUsageError {
                    message: format!(
                        "unknown option --check-hash-based-pycs={val}\n{}",
                        CliUsageError::usage_line(prog)
                    ),
                    exit_code: 2,
                });
            }
            config.check_hash_based_pycs = val.to_string();
            i += 1;
            continue;
        }
        if arg == "--emit" {
            action = Some(match args.get(i + 1).map(String::as_str) {
                Some("mir") => match args.get(i + 2) {
                    Some(path) => CliAction::EmitMir { path: path.clone() },
                    None => {
                        return Err(CliUsageError {
                            message: "usage: pyrust --emit mir <file.py>".to_string(),
                            exit_code: 2,
                        });
                    }
                },
                Some(other) => {
                    return Err(CliUsageError {
                        message: format!(
                            "Unknown --emit target: {other}\nusage: pyrust --emit mir <file.py>"
                        ),
                        exit_code: 2,
                    });
                }
                None => return Err(CliUsageError::missing_arg("--emit", prog)),
            });
            break;
        }
        if let Some(target) = arg.strip_prefix("--emit=") {
            if target == "mir" {
                action = Some(match args.get(i + 1) {
                    Some(path) => CliAction::EmitMir { path: path.clone() },
                    None => {
                        return Err(CliUsageError {
                            message: "usage: pyrust --emit=mir <file.py>".to_string(),
                            exit_code: 2,
                        });
                    }
                });
                break;
            }
            return Err(CliUsageError {
                message: format!(
                    "Unknown --emit target: {target}\nusage: pyrust --emit=mir <file.py>"
                ),
                exit_code: 2,
            });
        }
        if arg.starts_with("--") {
            return Err(CliUsageError::unknown_option(arg, prog));
        }
        if arg.starts_with('-') {
            let mut j = 1;
            let bytes = arg.as_bytes();
            let mut consumed_next = false;
            let mut terminated = false;
            while j < bytes.len() {
                match bytes[j] {
                    b'b' => {
                        config.bytes_warning += 1;
                        j += 1;
                    }
                    b'B' => {
                        config.dont_write_bytecode += 1;
                        j += 1;
                    }
                    b'd' => {
                        config.debug += 1;
                        j += 1;
                    }
                    b'E' => {
                        config.ignore_environment += 1;
                        j += 1;
                    }
                    b'h' | b'?' => {
                        help_mode = Some(HelpMode::Main);
                        j += 1;
                    }
                    b'i' => {
                        config.inspect += 1;
                        j += 1;
                    }
                    b'I' => {
                        config.isolated += 1;
                        config.ignore_environment += 1;
                        config.no_user_site += 1;
                        config.safe_path = true;
                        j += 1;
                    }
                    b'O' => {
                        config.optimize += 1;
                        j += 1;
                    }
                    b'P' => {
                        config.safe_path = true;
                        j += 1;
                    }
                    b'q' => {
                        config.quiet += 1;
                        j += 1;
                    }
                    b's' => {
                        config.no_user_site += 1;
                        j += 1;
                    }
                    b'S' => {
                        config.no_site += 1;
                        j += 1;
                    }
                    b'u' => {
                        config.unbuffered = true;
                        j += 1;
                    }
                    b'v' => {
                        config.verbose += 1;
                        j += 1;
                    }
                    b'V' => {
                        version_count += 1;
                        j += 1;
                    }
                    b'x' => {
                        config.skip_first_line = true;
                        j += 1;
                    }
                    b'W' => {
                        let opt = if j + 1 < arg.len() {
                            arg[j + 1..].to_string()
                        } else if i + 1 < args.len() {
                            consumed_next = true;
                            args[i + 1].clone()
                        } else {
                            return Err(CliUsageError::missing_arg("-W", prog));
                        };
                        cli_warnoptions.push(opt);
                        break;
                    }
                    b'X' => {
                        let opt = if j + 1 < arg.len() {
                            arg[j + 1..].to_string()
                        } else if i + 1 < args.len() {
                            consumed_next = true;
                            args[i + 1].clone()
                        } else {
                            return Err(CliUsageError::missing_arg("-X", prog));
                        };
                        if let Some((k, v)) = opt.split_once('=') {
                            config.xoptions.push((k.to_string(), Some(v.to_string())));
                        } else {
                            config.xoptions.push((opt, None));
                        }
                        break;
                    }
                    b'c' => {
                        let (command, rest) = if j + 1 < arg.len() {
                            (arg[j + 1..].to_string(), args[i + 1..].to_vec())
                        } else if i + 1 < args.len() {
                            (args[i + 1].clone(), args[i + 2..].to_vec())
                        } else {
                            return Err(CliUsageError::missing_arg("-c", prog));
                        };
                        action = Some(CliAction::RunCommand {
                            command,
                            args: rest,
                        });
                        terminated = true;
                        break;
                    }
                    b'm' => {
                        let (module, rest) = if j + 1 < arg.len() {
                            (arg[j + 1..].to_string(), args[i + 1..].to_vec())
                        } else if i + 1 < args.len() {
                            (args[i + 1].clone(), args[i + 2..].to_vec())
                        } else {
                            return Err(CliUsageError::missing_arg("-m", prog));
                        };
                        action = Some(CliAction::RunModule { module, args: rest });
                        terminated = true;
                        break;
                    }
                    _ => {
                        let ch = arg[j..].chars().next().expect("non-empty option tail");
                        return Err(CliUsageError::unknown_option(&format!("-{ch}"), prog));
                    }
                }
            }
            if terminated {
                break;
            }
            i += if consumed_next { 2 } else { 1 };
            continue;
        }
        action = Some(CliAction::RunFile {
            path: arg.clone(),
            args: args[i + 1..].to_vec(),
        });
        break;
    }

    if let Some(mode) = help_mode {
        let act = match mode {
            HelpMode::Main => CliAction::PrintHelp,
            HelpMode::Env => CliAction::PrintHelpEnv,
            HelpMode::XOptions => CliAction::PrintHelpXOptions,
            HelpMode::All => CliAction::PrintHelpAll,
        };
        return Ok(ParsedCli {
            action: act,
            config,
        });
    }
    if version_count > 0 {
        return Ok(ParsedCli {
            action: CliAction::PrintVersion {
                verbose: version_count >= 2,
            },
            config,
        });
    }

    let mut env_warnoptions: Vec<String> = Vec::new();
    if config.ignore_environment == 0 && config.isolated == 0 {
        if let Some(v) = get_env("PYTHONDEBUG") {
            config.debug += parse_env_count(&v);
        }
        if let Some(v) = get_env("PYTHONINSPECT")
            && !v.is_empty()
            && config.inspect == 0
        {
            config.inspect = 1;
        }
        if let Some(v) = get_env("PYTHONOPTIMIZE") {
            config.optimize += parse_env_count(&v);
        }
        if let Some(v) = get_env("PYTHONDONTWRITEBYTECODE")
            && !v.is_empty()
            && config.dont_write_bytecode == 0
        {
            config.dont_write_bytecode = 1;
        }
        if let Some(v) = get_env("PYTHONNOUSERSITE")
            && !v.is_empty()
            && config.no_user_site == 0
        {
            config.no_user_site = 1;
        }
        if let Some(v) = get_env("PYTHONVERBOSE") {
            config.verbose += parse_env_count(&v);
        }
        if let Some(v) = get_env("PYTHONUNBUFFERED")
            && !v.is_empty()
        {
            config.unbuffered = true;
        }
        if let Some(v) = get_env("PYTHONSAFEPATH")
            && !v.is_empty()
        {
            config.safe_path = true;
        }
        if let Some(v) = get_env("PYTHONDEVMODE")
            && !v.is_empty()
        {
            config.dev_mode = true;
        }
        if let Some(v) = get_env("PYTHONWARNDEFAULTENCODING")
            && !v.is_empty()
        {
            config.warn_default_encoding = 1;
        }
        if let Some(v) = get_env("PYTHONUTF8")
            && !v.is_empty()
        {
            match v.as_str() {
                "1" => config.utf8_mode = 1,
                "0" => config.utf8_mode = 0,
                _ => {
                    return Err(CliUsageError::fatal(
                        "Fatal Python error: config_init_utf8_mode: invalid PYTHONUTF8 environment variable value",
                    ));
                }
            }
        }
        if let Some(v) = get_env("PYTHONINTMAXSTRDIGITS")
            && !v.is_empty()
        {
            config.int_max_str_digits = validate_int_max_str_digits(&v, "PYTHONINTMAXSTRDIGITS")?;
        }
        if let Some(v) = get_env("PYTHONPYCACHEPREFIX")
            && !v.is_empty()
        {
            config.pycache_prefix = Some(v);
        }
        if let Some(v) = get_env("PYTHONPLATLIBDIR")
            && !v.is_empty()
        {
            config.platlibdir = Some(v);
        }
        if let Some(v) = get_env("PYTHONPATH")
            && !v.is_empty()
        {
            let sep = if cfg!(windows) { ';' } else { ':' };
            for part in v.split(sep) {
                config.python_path.push(part.to_string());
            }
        }
        if let Some(v) = get_env("PYTHONHOME")
            && !v.is_empty()
        {
            let sep = if cfg!(windows) { ';' } else { ':' };
            if let Some((prefix, exec_prefix)) = v.split_once(sep) {
                config.prefix_override = Some(prefix.to_string());
                config.exec_prefix_override = Some(exec_prefix.to_string());
            } else {
                config.prefix_override = Some(v);
            }
        }
        if let Some(v) = get_env("PYTHONHASHSEED")
            && !v.is_empty()
        {
            if v == "0" {
                config.hash_randomization = 0;
            } else {
                config.hash_randomization = 1;
            }
        }
        if let Some(v) = get_env("PYTHONWARNINGS")
            && !v.is_empty()
        {
            for part in v.split(',') {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    env_warnoptions.push(trimmed.to_string());
                }
            }
        }
    }

    // Apply CLI `-X` options (CLI overrides corresponding `PYTHON*` env vars).
    for (key, val) in &config.xoptions {
        match key.as_str() {
            "dev" => {
                config.dev_mode = true;
            }
            "utf8" => match val.as_deref() {
                None | Some("1") => config.utf8_mode = 1,
                Some("0") => config.utf8_mode = 0,
                Some(_) => {
                    return Err(CliUsageError::fatal(
                        "Fatal Python error: config_init_utf8_mode: invalid -X utf8 option value",
                    ));
                }
            },
            "warn_default_encoding" => {
                config.warn_default_encoding = 1;
            }
            "int_max_str_digits" => match val.as_deref() {
                Some(raw) => {
                    config.int_max_str_digits =
                        validate_int_max_str_digits(raw, "-X int_max_str_digits")?;
                }
                None => {
                    return Err(CliUsageError::fatal(
                        "Fatal Python error: init_sys_config: -X int_max_str_digits: invalid limit; \
                         must be >= 640 or 0 for unlimited.",
                    ));
                }
            },
            "pycache_prefix" => {
                config.pycache_prefix = val.as_ref().filter(|s| !s.is_empty()).cloned();
            }
            _ => {}
        }
    }

    if config.dev_mode {
        config.warnoptions.push("default".to_string());
    }
    config.warnoptions.extend(env_warnoptions);
    config.warnoptions.extend(cli_warnoptions);

    let resolved_action = action.unwrap_or(CliAction::Repl);
    if matches!(resolved_action, CliAction::Repl) {
        config.interactive = 1;
    }

    Ok(ParsedCli {
        action: resolved_action,
        config,
    })
}

fn parse_cli_args(args: &[String]) -> std::result::Result<ParsedCli, CliUsageError> {
    let mut parsed = parse_cli_args_with_env(args, |key| std::env::var(key).ok())?;
    // PEP 405 virtual environment startup detection: when `PYTHONHOME` is not
    // explicitly overriding prefixes, check the executable's directory and its
    // parent directory for `pyvenv.cfg`.
    if parsed.config.prefix_override.is_none()
        && let Some(venv_info) =
            builtin_modules::sys::detect_pyvenv_startup(args.first().map(String::as_str))
    {
        parsed.config.venv_prefix = Some(venv_info.prefix);
        parsed.config.venv_base_prefix = Some(venv_info.base_prefix);
        parsed.config.venv_executable = Some(venv_info.executable);
        parsed.config.venv_base_executable = Some(venv_info.base_executable);
        parsed.config.venv_home = venv_info.home;
        parsed.config.venv_include_system_site_packages =
            Some(venv_info.include_system_site_packages);
    }
    Ok(parsed)
}

fn parse_source(src: &str) -> Result<Vec<ast::Stmt>> {
    let tokens = Lexer::new(src)?.into_tokens();
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

// Interpreter-thread stack, sized so deep Python recursion (up to the default
// `RecursionError` limit of 1000 calls) never overflows the OS default (1 MB on
// Windows) with comfortable margin for an elevated `sys.setrecursionlimit()`.
//
// The release VM frame for the worst recursion shape (a `try`/`except` body
// driven through the native memoized-call path) is ~52 KB, so 1000-deep
// recursion peaks at ~56 MB; 128 MB gives ~2.3× headroom and supports an
// elevated limit up to ~2000 in the worst case (far more for ordinary
// recursion).  Halving the reservation from the previous 256 MB cuts the
// worst-case committed memory and virtual footprint under deep recursion in two,
// with zero effect on normal programs (the stack is committed lazily, so a
// non-recursing script still resides in ~5 MB).
//
// Debug builds reserve 512 MB: recursive Python dunder dispatch traverses
// enough unoptimised VM and slot frames to exhaust 256 MB before the
// `CallDepthGuard` reaches the default limit of 1000.  Doubling the reservation
// lets the semantic guard fire with headroom for CI's debug parity suite.  The
// stack is committed lazily, so normal non-recursing resident memory stays low.
#[cfg(debug_assertions)]
const INTERPRETER_STACK_SIZE: usize = 512 * 1024 * 1024;
#[cfg(not(debug_assertions))]
const INTERPRETER_STACK_SIZE: usize = 128 * 1024 * 1024;

fn run_on_interpreter_thread<F>(f: F) -> Result<()>
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    // Marshal errors as strings so the Result crosses the thread boundary
    // (Value contains Rc which is not Send).
    //
    // `PyError::Runtime` is used by `script.rs` to carry a pre-formatted
    // traceback string (the "Traceback (most recent call last): …" header
    // plus the error line).  Extract the inner message directly instead of
    // going through `Display` (which would prepend "Runtime error: ").
    let err_str: Option<String> = std::thread::Builder::new()
        .stack_size(INTERPRETER_STACK_SIZE)
        .spawn(move || {
            f().err().map(|e| match e.into_kind() {
                crate::error::PyErrorKind::Runtime(msg) => msg,
                other => other.to_string(),
            })
        })
        .expect("failed to spawn interpreter thread")
        .join()
        .expect("interpreter thread panicked");

    if let Some(msg) = err_str {
        return Err(PyError::runtime(msg));
    }
    Ok(())
}

fn finish_cli_interpreter(
    interp: &mut Interpreter,
    result: Result<()>,
    inspect: bool,
) -> Result<()> {
    if inspect {
        if let Err(err) = result {
            match err.kind() {
                crate::error::PyErrorKind::Runtime(msg) => eprintln!("{msg}"),
                other => eprintln!("{other}"),
            }
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
        run_repl_loop(interp)?;
        interp.run_atexit_callbacks();
        return Ok(());
    }
    if let Err(err) = result {
        if interp.features().atexit_len() != 0 {
            match err.kind() {
                crate::error::PyErrorKind::Runtime(msg) => eprintln!("{msg}"),
                other => eprintln!("{other}"),
            }
            let _ = std::io::Write::flush(&mut std::io::stderr());
            interp.run_atexit_callbacks();
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let _ = std::io::Write::flush(&mut std::io::stderr());
            std::process::exit(1);
        }
        return Err(err);
    }
    Ok(())
}

fn strip_first_line(src: &str) -> String {
    match src.find('\n') {
        Some(idx) => src[idx..].to_string(),
        None => String::new(),
    }
}

fn run_file(path: &str, script_args: &[String], config: RuntimeConfig) -> Result<()> {
    let raw_src = std::fs::read_to_string(path)
        .map_err(|e| PyError::runtime(format!("failed to read '{path}': {e}")))?;
    let src = if config.skip_first_line {
        strip_first_line(&raw_src)
    } else {
        raw_src
    };
    let path_owned = path.to_string();
    let script_args = script_args.to_vec();
    let inspect = config.inspect > 0;

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_script_path_and_args(&path_owned, &script_args);
        interp.set_runtime_config(config);
        interp.initialize_cli_startup()?;
        let (program, linenos) = interp.parse_source_to_stmts_with_warnings(&src, &path_owned)?;
        let res = interp.exec_program_with_linenos(&program, &linenos, &src, false);
        finish_cli_interpreter(&mut interp, res, inspect)
    })
}

fn run_command(command: &str, script_args: &[String], config: RuntimeConfig) -> Result<()> {
    let src = command.to_string();
    let script_args = script_args.to_vec();
    let inspect = config.inspect > 0;

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_command_args(&script_args);
        interp.set_runtime_config(config);
        interp.initialize_cli_startup()?;
        let (program, linenos) = interp.parse_source_to_stmts_with_warnings(&src, "<string>")?;
        let res = interp.exec_program_with_linenos(&program, &linenos, &src, false);
        finish_cli_interpreter(&mut interp, res, inspect)
    })
}

fn run_stdin(script_args: &[String], config: RuntimeConfig) -> Result<()> {
    let mut src = String::new();
    std::io::stdin()
        .read_to_string(&mut src)
        .map_err(|e| PyError::runtime(format!("failed to read stdin: {e}")))?;
    let script_args = script_args.to_vec();
    let inspect = config.inspect > 0;

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_stdin_args(&script_args);
        interp.set_runtime_config(config);
        interp.initialize_cli_startup()?;
        let (program, linenos) = interp.parse_source_to_stmts_with_warnings(&src, "<stdin>")?;
        let res = interp.exec_program_with_linenos(&program, &linenos, &src, false);
        finish_cli_interpreter(&mut interp, res, inspect)
    })
}

fn run_module(
    module: &str,
    script_args: &[String],
    prog: &str,
    config: RuntimeConfig,
) -> Result<()> {
    let module_owned = module.to_string();
    let script_args = script_args.to_vec();
    let prog_owned = prog.to_string();
    let inspect = config.inspect > 0;

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_module_args(&script_args);
        interp.set_runtime_config(config);
        interp.initialize_cli_startup()?;
        let res = interp.run_module_as_main(&module_owned, &prog_owned);
        finish_cli_interpreter(&mut interp, res, inspect)
    })
}

fn last_line_is_indented(buf: &str) -> bool {
    buf.lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| l.starts_with([' ', '\t']))
        .unwrap_or(false)
}

fn is_incomplete(err: &PyError) -> bool {
    match err.kind() {
        // "found Eof" — input cut off mid-expression
        // "expected Indent, found" — block header (e.g. "for x:") with no body yet;
        //   the trailing \n produces an extra Newline token instead of Eof
        crate::error::PyErrorKind::Parse(msg) | crate::error::PyErrorKind::Lex(msg) => {
            msg.contains("found Eof") || msg.contains("expected Indent, found")
        }
        _ => false,
    }
}

fn normalize_repl_error(error: PyError) -> PyError {
    lex_parse_to_exc(error)
}

fn run_repl_loop(interpreter: &mut Interpreter) -> Result<()> {
    let mut rl = DefaultEditor::new().map_err(|e| PyError::runtime(e.to_string()))?;
    let mut buf = String::new();

    loop {
        let prompt = if buf.is_empty() { ">>> " } else { "... " };
        let line = match rl.readline(prompt) {
            Ok(l) => l,
            Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
            Err(e) => {
                eprintln!("{e}");
                break;
            }
        };

        let trimmed = line.trim();
        if buf.is_empty() && (trimmed == "exit" || trimmed == "quit") {
            break;
        }

        // Empty line while in a block flushes the buffer
        if !buf.is_empty() && trimmed.is_empty() {
            let src = std::mem::take(&mut buf);
            match parse_source(&src) {
                Ok(program) => {
                    if let Err(e) = interpreter.exec_program(&program, true) {
                        eprintln!("{e}");
                    }
                }
                Err(e) => eprintln!("{}", normalize_repl_error(e)),
            }
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        // Add each line to history individually (not the whole buffer) so
        // recalled lines are displayed one at a time with proper indentation.
        rl.add_history_entry(&line).ok();

        buf.push_str(&line);
        buf.push('\n');

        // Lines ending with ':' always need a body — skip parse attempt
        if trimmed.ends_with(':') {
            continue;
        }

        // Try to parse and execute; stay in continuation if incomplete
        match parse_source(&buf) {
            Ok(program) if !last_line_is_indented(&buf) => {
                buf.clear();
                if let Err(e) = interpreter.exec_program(&program, true) {
                    eprintln!("{e}");
                }
            }
            Ok(_) => {
                // Last line is still indented — wait for empty line to flush
            }
            Err(e) if is_incomplete(&e) => {
                // Need more input — keep buf and show "..." next iteration
            }
            Err(e) => {
                buf.clear();
                eprintln!("{}", normalize_repl_error(e));
            }
        }
    }
    Ok(())
}

fn run_repl(config: RuntimeConfig) -> Result<()> {
    if config.quiet == 0 {
        println!("PyRust 0.2 (Python-like subset). Type 'exit' or 'quit' to leave.");
    }

    run_on_interpreter_thread(move || {
        let mut interpreter = Interpreter::default();
        interpreter.set_runtime_config(config);
        interpreter.initialize_cli_startup()?;
        run_repl_loop(&mut interpreter)?;
        interpreter.run_atexit_callbacks();
        Ok(())
    })
}

fn emit_mir_file(path: &str) -> Result<()> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| PyError::runtime(format!("failed to read '{path}': {e}")))?;
    let rendered = mir::compile_source_to_mir_with_filename(&src, path)?;
    print!("{rendered}");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let prog = args.first().map(String::as_str).unwrap_or("pyrust");
    let parsed = match parse_cli_args(&args) {
        Ok(parsed) => parsed,
        Err(usage_err) => {
            eprintln!("{}", usage_err.message);
            std::process::exit(usage_err.exit_code);
        }
    };

    let result = match parsed.action {
        CliAction::PrintVersion { verbose } => {
            println!("{}", format_version(verbose));
            Ok(())
        }
        CliAction::PrintHelp => {
            println!("{}", format_help(prog));
            Ok(())
        }
        CliAction::PrintHelpEnv => {
            println!("{}", format_help_env());
            Ok(())
        }
        CliAction::PrintHelpXOptions => {
            println!("{}", format_help_xoptions());
            Ok(())
        }
        CliAction::PrintHelpAll => {
            println!(
                "{}\n\n{}\n\n{}",
                format_help(prog),
                format_help_env(),
                format_help_xoptions()
            );
            Ok(())
        }
        CliAction::EmitMir { path } => emit_mir_file(&path),
        CliAction::RunFile { path, args } => run_file(&path, &args, parsed.config),
        CliAction::RunCommand { command, args } => run_command(&command, &args, parsed.config),
        CliAction::RunModule { module, args } => run_module(&module, &args, prog, parsed.config),
        CliAction::RunStdin { args } => run_stdin(&args, parsed.config),
        CliAction::Repl => run_repl(parsed.config),
    };

    if let Err(e) = result {
        // PyError::Runtime already carries a fully-formatted message (e.g.
        // "SyntaxError: …" from the interpreter); print it raw so that we
        // don't prepend an extra "Runtime error: " prefix via Display.
        match e.kind() {
            crate::error::PyErrorKind::Runtime(msg) => eprintln!("{msg}"),
            other => eprintln!("{other}"),
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── last_line_is_indented ────────────────────────────────────────────────

    #[test]
    fn empty_buffer_is_not_indented() {
        assert!(!last_line_is_indented(""));
    }

    #[test]
    fn single_unindented_line_is_not_indented() {
        assert!(!last_line_is_indented("x = 1\n"));
    }

    #[test]
    fn single_indented_line_is_indented() {
        assert!(last_line_is_indented("    return 1\n"));
    }

    #[test]
    fn last_line_indented_after_header() {
        assert!(last_line_is_indented("for i in range(3):\n    print(i)\n"));
    }

    #[test]
    fn last_line_unindented_means_block_is_done() {
        assert!(!last_line_is_indented(
            "for i in range(3):\n    print(i)\nresult = 1\n"
        ));
    }

    #[test]
    fn blank_trailing_line_looks_through_to_indented_line() {
        // buffer has a trailing blank — last *non-empty* line is still indented
        assert!(last_line_is_indented(
            "for i in range(3):\n    print(i)\n\n"
        ));
    }

    #[test]
    fn tab_indentation_is_detected() {
        assert!(last_line_is_indented("def f():\n\treturn 1\n"));
    }

    // ── is_incomplete ────────────────────────────────────────────────────────

    #[test]
    fn eof_parse_error_is_incomplete() {
        assert!(is_incomplete(&PyError::parse(
            "expected X, found Eof".into()
        )));
    }

    #[test]
    fn expected_indent_parse_error_is_incomplete() {
        // produced when a block header line ends with '\n' and the lexer emits
        // an extra Newline token instead of Eof
        assert!(is_incomplete(&PyError::parse(
            "expected Indent, found Some(Newline)".into()
        )));
    }

    #[test]
    fn eof_lex_error_is_incomplete() {
        assert!(is_incomplete(&PyError::lex("found Eof".into())));
    }

    #[test]
    fn unrelated_parse_error_is_not_incomplete() {
        assert!(!is_incomplete(&PyError::parse(
            "unexpected token in expression: Some(Plus)".into()
        )));
    }

    #[test]
    fn unclosed_delimiter_is_not_newly_treated_as_incomplete() {
        let err = parse_source("x = (1 +\n").unwrap_err();
        assert_eq!(err.to_string(), "Parse error: '(' was never closed");
        assert!(!is_incomplete(&err));
    }

    #[test]
    fn runtime_error_is_not_incomplete() {
        assert!(!is_incomplete(&PyError::runtime("name error".into())));
    }

    #[test]
    fn repl_parse_error_is_normalized_for_display() {
        let error = normalize_repl_error(PyError::parse("invalid syntax".into()));
        assert_eq!(error.to_string(), "SyntaxError: invalid syntax");
    }

    #[test]
    fn repl_runtime_error_is_not_reclassified_for_display() {
        let error = normalize_repl_error(PyError::runtime("execution failed".into()));
        assert_eq!(error.to_string(), "Runtime error: execution failed");
    }

    // ── integration: parse_source drives is_incomplete ───────────────────────

    #[test]
    fn block_header_only_is_incomplete() {
        // "for i in range(10):\n" — no body yet
        let err = parse_source("for i in range(10):\n").unwrap_err();
        assert!(is_incomplete(&err));
    }

    #[test]
    fn nested_block_header_is_incomplete() {
        let err = parse_source("for i in range(10):\n    for j in range(20):\n").unwrap_err();
        assert!(is_incomplete(&err));
    }

    #[test]
    fn complete_single_line_is_not_incomplete() {
        assert!(parse_source("x = 1 + 2\n").is_ok());
    }

    #[test]
    fn complete_block_parses_ok_with_indented_last_line() {
        // parse succeeds but last line is indented — REPL should wait for empty line
        let src = "for i in range(3):\n    print(i)\n";
        assert!(parse_source(src).is_ok());
        assert!(last_line_is_indented(src));
    }

    #[test]
    fn def_with_body_parses_ok_but_still_indented() {
        let src = "def f():\n    return 42\n";
        assert!(parse_source(src).is_ok());
        assert!(last_line_is_indented(src));
    }

    // ── parse_cli_args ───────────────────────────────────────────────────────

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn parse_no_env(items: &[&str]) -> std::result::Result<ParsedCli, CliUsageError> {
        parse_cli_args_with_env(&argv(items), |_| None)
    }

    #[test]
    fn cli_no_args_starts_repl() {
        let parsed = parse_no_env(&["pyrust"]).unwrap();
        assert_eq!(parsed.action, CliAction::Repl);
        assert_eq!(parsed.config.interactive, 1);
    }

    #[test]
    fn cli_command_flag_and_attached_form() {
        assert_eq!(
            parse_no_env(&["pyrust", "-c", "print(1)", "-V", "--foo"])
                .unwrap()
                .action,
            CliAction::RunCommand {
                command: "print(1)".into(),
                args: argv(&["-V", "--foo"]),
            }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "-cprint(1)", "arg1"])
                .unwrap()
                .action,
            CliAction::RunCommand {
                command: "print(1)".into(),
                args: argv(&["arg1"]),
            }
        );
    }

    #[test]
    fn cli_module_flag_and_attached_form() {
        assert_eq!(
            parse_no_env(&["pyrust", "-m", "pkg.mod", "--flag", "x"])
                .unwrap()
                .action,
            CliAction::RunModule {
                module: "pkg.mod".into(),
                args: argv(&["--flag", "x"]),
            }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "-mpkg.mod", "x"]).unwrap().action,
            CliAction::RunModule {
                module: "pkg.mod".into(),
                args: argv(&["x"]),
            }
        );
    }

    #[test]
    fn cli_version_flags() {
        assert_eq!(
            parse_no_env(&["pyrust", "-V"]).unwrap().action,
            CliAction::PrintVersion { verbose: false }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "--version"]).unwrap().action,
            CliAction::PrintVersion { verbose: false }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "-VV"]).unwrap().action,
            CliAction::PrintVersion { verbose: true }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "-V", "-V"]).unwrap().action,
            CliAction::PrintVersion { verbose: true }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "-V", "-c", "print(1)"])
                .unwrap()
                .action,
            CliAction::PrintVersion { verbose: false }
        );
    }

    #[test]
    fn cli_double_dash_and_stdin() {
        assert_eq!(
            parse_no_env(&["pyrust", "--", "-weird.py", "--arg"])
                .unwrap()
                .action,
            CliAction::RunFile {
                path: "-weird.py".into(),
                args: argv(&["--arg"]),
            }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "-", "a", "--b"]).unwrap().action,
            CliAction::RunStdin {
                args: argv(&["a", "--b"]),
            }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "--", "-", "a"]).unwrap().action,
            CliAction::RunStdin { args: argv(&["a"]) }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "--"]).unwrap().action,
            CliAction::Repl
        );
    }

    #[test]
    fn cli_script_stops_option_parsing_and_preserves_emit_mir() {
        assert_eq!(
            parse_no_env(&["pyrust", "script.py", "-c", "foo", "-V"])
                .unwrap()
                .action,
            CliAction::RunFile {
                path: "script.py".into(),
                args: argv(&["-c", "foo", "-V"]),
            }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "--emit", "mir", "a.py"])
                .unwrap()
                .action,
            CliAction::EmitMir {
                path: "a.py".into()
            }
        );
        assert_eq!(
            parse_no_env(&["pyrust", "--emit=mir", "a.py"])
                .unwrap()
                .action,
            CliAction::EmitMir {
                path: "a.py".into()
            }
        );
    }

    #[test]
    fn cli_clustered_cpython_flags_and_env_vars() {
        let parsed = parse_no_env(&[
            "pyrust",
            "-OOBuS",
            "-bb",
            "-vv",
            "-P",
            "-q",
            "-d",
            "-x",
            "-Wignore",
            "-W",
            "error::DeprecationWarning",
            "-Xdev",
            "-X",
            "int_max_str_digits=5000",
            "-X",
            "pycache_prefix=/tmp/pyc",
            "-Xutf8=1",
            "--check-hash-based-pycs=always",
            "-c",
            "pass",
        ])
        .unwrap();
        let cfg = &parsed.config;
        assert_eq!(cfg.optimize, 2);
        assert_eq!(cfg.dont_write_bytecode, 1);
        assert!(cfg.unbuffered);
        assert_eq!(cfg.no_site, 1);
        assert_eq!(cfg.bytes_warning, 2);
        assert_eq!(cfg.verbose, 2);
        assert!(cfg.safe_path);
        assert_eq!(cfg.quiet, 1);
        assert_eq!(cfg.debug, 1);
        assert!(cfg.skip_first_line);
        assert!(cfg.dev_mode);
        assert_eq!(cfg.int_max_str_digits, 5000);
        assert_eq!(cfg.pycache_prefix.as_deref(), Some("/tmp/pyc"));
        assert_eq!(cfg.utf8_mode, 1);
        assert_eq!(cfg.check_hash_based_pycs, "always");
        assert_eq!(
            cfg.warnoptions,
            vec!["default", "ignore", "error::DeprecationWarning"]
        );

        // Env vars honored normally, ignored with -E and -I.
        let with_env = parse_cli_args_with_env(&argv(&["pyrust", "-c", "pass"]), |k| match k {
            "PYTHONOPTIMIZE" => Some("2".into()),
            "PYTHONDONTWRITEBYTECODE" => Some("1".into()),
            "PYTHONSAFEPATH" => Some("1".into()),
            "PYTHONWARNINGS" => Some("error,ignore::DeprecationWarning".into()),
            "PYTHONINTMAXSTRDIGITS" => Some("6000".into()),
            "PYTHONHASHSEED" => Some("0".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(with_env.config.optimize, 2);
        assert_eq!(with_env.config.dont_write_bytecode, 1);
        assert!(with_env.config.safe_path);
        assert_eq!(
            with_env.config.warnoptions,
            vec!["error", "ignore::DeprecationWarning"]
        );
        assert_eq!(with_env.config.int_max_str_digits, 6000);
        assert_eq!(with_env.config.hash_randomization, 0);

        let with_e = parse_cli_args_with_env(&argv(&["pyrust", "-E", "-c", "pass"]), |k| match k {
            "PYTHONOPTIMIZE" => Some("2".into()),
            "PYTHONSAFEPATH" => Some("1".into()),
            "PYTHONWARNINGS" => Some("error".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(with_e.config.ignore_environment, 1);
        assert_eq!(with_e.config.optimize, 0);
        assert!(!with_e.config.safe_path);
        assert!(with_e.config.warnoptions.is_empty());

        let with_i = parse_cli_args_with_env(&argv(&["pyrust", "-I", "-c", "pass"]), |k| match k {
            "PYTHONOPTIMIZE" => Some("2".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(with_i.config.isolated, 1);
        assert_eq!(with_i.config.ignore_environment, 1);
        assert_eq!(with_i.config.no_user_site, 1);
        assert!(with_i.config.safe_path);
        assert_eq!(with_i.config.optimize, 0);
    }

    #[test]
    fn cli_help_variants_and_usage_errors() {
        assert_eq!(
            parse_no_env(&["pyrust", "--help-env"]).unwrap().action,
            CliAction::PrintHelpEnv
        );
        assert_eq!(
            parse_no_env(&["pyrust", "--help-xoptions"]).unwrap().action,
            CliAction::PrintHelpXOptions
        );
        assert_eq!(
            parse_no_env(&["pyrust", "--help-all"]).unwrap().action,
            CliAction::PrintHelpAll
        );

        let missing_c = parse_no_env(&["pyrust", "-c"]).unwrap_err();
        assert_eq!(missing_c.exit_code, 2);
        assert!(missing_c.message.contains("-c"));

        let missing_m = parse_no_env(&["pyrust", "-m"]).unwrap_err();
        assert_eq!(missing_m.exit_code, 2);
        assert!(missing_m.message.contains("-m"));

        let missing_w = parse_no_env(&["pyrust", "-W"]).unwrap_err();
        assert_eq!(missing_w.exit_code, 2);
        assert!(missing_w.message.contains("-W"));

        let missing_x = parse_no_env(&["pyrust", "-X"]).unwrap_err();
        assert_eq!(missing_x.exit_code, 2);
        assert!(missing_x.message.contains("-X"));

        let bad_pyc = parse_no_env(&["pyrust", "--check-hash-based-pycs=bogus"]).unwrap_err();
        assert_eq!(bad_pyc.exit_code, 2);

        let unknown_short = parse_no_env(&["pyrust", "-Z"]).unwrap_err();
        assert_eq!(unknown_short.exit_code, 2);
        assert!(unknown_short.message.contains("Unknown option: -Z"));

        let unknown_long = parse_no_env(&["pyrust", "--bogus"]).unwrap_err();
        assert_eq!(unknown_long.exit_code, 2);
        assert!(unknown_long.message.contains("Unknown option: --bogus"));

        let version_then_unknown = parse_no_env(&["pyrust", "-V", "--bogus"]).unwrap_err();
        assert_eq!(version_then_unknown.exit_code, 2);
    }
}
