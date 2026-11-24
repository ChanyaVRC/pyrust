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
use interpreter::{Interpreter, lex_parse_to_exc};
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
    EmitMir { path: String },
    RunFile { path: String, args: Vec<String> },
    RunCommand { command: String, args: Vec<String> },
    RunModule { module: String, args: Vec<String> },
    RunStdin { args: Vec<String> },
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
         Options:\n\
         -c cmd      : program passed in as string (terminates option list)\n\
         -h, --help  : print this help message and exit\n\
         -m mod      : run library module as a script (terminates option list)\n\
         -V          : print the version number and exit (also --version)\n\
         -VV         : print more detailed version information and exit\n\
         --emit mir  : compile a file to SSA MIR and print it\n\
         --          : terminate option list\n\
         Arguments:\n\
         file        : program read from script file\n\
         -           : program read from stdin\n\
         arg ...     : arguments passed to program in sys.argv[1:]"
    )
}

fn parse_cli_args(args: &[String]) -> std::result::Result<CliAction, CliUsageError> {
    let prog = args.first().map(String::as_str).unwrap_or("pyrust");
    let mut version_count: usize = 0;
    let mut show_help = false;
    let mut i = 1;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            if show_help {
                return Ok(CliAction::PrintHelp);
            }
            if version_count > 0 {
                return Ok(CliAction::PrintVersion {
                    verbose: version_count >= 2,
                });
            }
            return match args.get(i + 1) {
                None => Ok(CliAction::Repl),
                Some(next) if next == "-" => Ok(CliAction::RunStdin {
                    args: args[i + 2..].to_vec(),
                }),
                Some(path) => Ok(CliAction::RunFile {
                    path: path.clone(),
                    args: args[i + 2..].to_vec(),
                }),
            };
        }
        if arg == "-" {
            if show_help {
                return Ok(CliAction::PrintHelp);
            }
            if version_count > 0 {
                return Ok(CliAction::PrintVersion {
                    verbose: version_count >= 2,
                });
            }
            return Ok(CliAction::RunStdin {
                args: args[i + 1..].to_vec(),
            });
        }
        if arg == "--version" {
            version_count += 1;
            i += 1;
            continue;
        }
        if arg == "--help" {
            show_help = true;
            i += 1;
            continue;
        }
        if arg == "--emit" {
            return match args.get(i + 1).map(String::as_str) {
                Some("mir") => match args.get(i + 2) {
                    Some(path) => Ok(CliAction::EmitMir { path: path.clone() }),
                    None => Err(CliUsageError {
                        message: "usage: pyrust --emit mir <file.py>".to_string(),
                        exit_code: 2,
                    }),
                },
                Some(other) => Err(CliUsageError {
                    message: format!(
                        "Unknown --emit target: {other}\nusage: pyrust --emit mir <file.py>"
                    ),
                    exit_code: 2,
                }),
                None => Err(CliUsageError::missing_arg("--emit", prog)),
            };
        }
        if let Some(target) = arg.strip_prefix("--emit=") {
            if target == "mir" {
                return match args.get(i + 1) {
                    Some(path) => Ok(CliAction::EmitMir { path: path.clone() }),
                    None => Err(CliUsageError {
                        message: "usage: pyrust --emit=mir <file.py>".to_string(),
                        exit_code: 2,
                    }),
                };
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
            while j < bytes.len() {
                match bytes[j] {
                    b'V' => {
                        version_count += 1;
                        j += 1;
                    }
                    b'h' | b'?' => {
                        show_help = true;
                        j += 1;
                    }
                    b'c' => {
                        let (command, rest) = if j + 1 < arg.len() {
                            (arg[j + 1..].to_string(), args[i + 1..].to_vec())
                        } else if i + 1 < args.len() {
                            (args[i + 1].clone(), args[i + 2..].to_vec())
                        } else {
                            return Err(CliUsageError::missing_arg("-c", prog));
                        };
                        if show_help {
                            return Ok(CliAction::PrintHelp);
                        }
                        if version_count > 0 {
                            return Ok(CliAction::PrintVersion {
                                verbose: version_count >= 2,
                            });
                        }
                        return Ok(CliAction::RunCommand {
                            command,
                            args: rest,
                        });
                    }
                    b'm' => {
                        let (module, rest) = if j + 1 < arg.len() {
                            (arg[j + 1..].to_string(), args[i + 1..].to_vec())
                        } else if i + 1 < args.len() {
                            (args[i + 1].clone(), args[i + 2..].to_vec())
                        } else {
                            return Err(CliUsageError::missing_arg("-m", prog));
                        };
                        if show_help {
                            return Ok(CliAction::PrintHelp);
                        }
                        if version_count > 0 {
                            return Ok(CliAction::PrintVersion {
                                verbose: version_count >= 2,
                            });
                        }
                        return Ok(CliAction::RunModule { module, args: rest });
                    }
                    _ => {
                        let ch = arg[j..].chars().next().expect("non-empty option tail");
                        return Err(CliUsageError::unknown_option(&format!("-{ch}"), prog));
                    }
                }
            }
            i += 1;
            continue;
        }
        if show_help {
            return Ok(CliAction::PrintHelp);
        }
        if version_count > 0 {
            return Ok(CliAction::PrintVersion {
                verbose: version_count >= 2,
            });
        }
        return Ok(CliAction::RunFile {
            path: arg.clone(),
            args: args[i + 1..].to_vec(),
        });
    }

    if show_help {
        return Ok(CliAction::PrintHelp);
    }
    if version_count > 0 {
        return Ok(CliAction::PrintVersion {
            verbose: version_count >= 2,
        });
    }
    Ok(CliAction::Repl)
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

fn run_file(path: &str, script_args: &[String]) -> Result<()> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| PyError::runtime(format!("failed to read '{path}': {e}")))?;
    let (program, linenos) = Interpreter::parse_source_to_stmts_with_linenos(&src)?;
    let path_owned = path.to_string();
    let script_args = script_args.to_vec();

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_script_path_and_args(&path_owned, &script_args);
        interp.exec_program_with_linenos(&program, &linenos, &src, false)
    })
}

fn run_command(command: &str, script_args: &[String]) -> Result<()> {
    let (program, linenos) = Interpreter::parse_source_to_stmts_with_linenos(command)?;
    let src = command.to_string();
    let script_args = script_args.to_vec();

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_command_args(&script_args);
        interp.exec_program_with_linenos(&program, &linenos, &src, false)
    })
}

fn run_stdin(script_args: &[String]) -> Result<()> {
    let mut src = String::new();
    std::io::stdin()
        .read_to_string(&mut src)
        .map_err(|e| PyError::runtime(format!("failed to read stdin: {e}")))?;
    let (program, linenos) = Interpreter::parse_source_to_stmts_with_linenos(&src)?;
    let script_args = script_args.to_vec();

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_stdin_args(&script_args);
        interp.exec_program_with_linenos(&program, &linenos, &src, false)
    })
}

fn run_module(module: &str, script_args: &[String], prog: &str) -> Result<()> {
    let module_owned = module.to_string();
    let script_args = script_args.to_vec();
    let prog_owned = prog.to_string();

    run_on_interpreter_thread(move || {
        let mut interp = Interpreter::with_module_args(&script_args);
        interp.run_module_as_main(&module_owned, &prog_owned)
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

fn run_repl() -> Result<()> {
    println!("PyRust 0.2 (Python-like subset). Type 'exit' or 'quit' to leave.");

    let mut rl = DefaultEditor::new().map_err(|e| PyError::runtime(e.to_string()))?;

    std::thread::Builder::new()
        .stack_size(INTERPRETER_STACK_SIZE)
        .spawn(move || {
            let mut interpreter = Interpreter::default();
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
        })
        .expect("failed to spawn REPL thread")
        .join()
        .expect("REPL thread panicked");

    Ok(())
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
    let action = match parse_cli_args(&args) {
        Ok(action) => action,
        Err(usage_err) => {
            eprintln!("{}", usage_err.message);
            std::process::exit(usage_err.exit_code);
        }
    };

    let result = match action {
        CliAction::PrintVersion { verbose } => {
            println!("{}", format_version(verbose));
            Ok(())
        }
        CliAction::PrintHelp => {
            println!("{}", format_help(prog));
            Ok(())
        }
        CliAction::EmitMir { path } => emit_mir_file(&path),
        CliAction::RunFile { path, args } => run_file(&path, &args),
        CliAction::RunCommand { command, args } => run_command(&command, &args),
        CliAction::RunModule { module, args } => run_module(&module, &args, prog),
        CliAction::RunStdin { args } => run_stdin(&args),
        CliAction::Repl => run_repl(),
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

    #[test]
    fn cli_no_args_starts_repl() {
        assert_eq!(parse_cli_args(&argv(&["pyrust"])), Ok(CliAction::Repl));
    }

    #[test]
    fn cli_command_flag_and_attached_form() {
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-c", "print(1)", "-V", "--foo"])),
            Ok(CliAction::RunCommand {
                command: "print(1)".into(),
                args: argv(&["-V", "--foo"]),
            })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-cprint(1)", "arg1"])),
            Ok(CliAction::RunCommand {
                command: "print(1)".into(),
                args: argv(&["arg1"]),
            })
        );
    }

    #[test]
    fn cli_module_flag_and_attached_form() {
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-m", "pkg.mod", "--flag", "x"])),
            Ok(CliAction::RunModule {
                module: "pkg.mod".into(),
                args: argv(&["--flag", "x"]),
            })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-mpkg.mod", "x"])),
            Ok(CliAction::RunModule {
                module: "pkg.mod".into(),
                args: argv(&["x"]),
            })
        );
    }

    #[test]
    fn cli_version_flags() {
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-V"])),
            Ok(CliAction::PrintVersion { verbose: false })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "--version"])),
            Ok(CliAction::PrintVersion { verbose: false })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-VV"])),
            Ok(CliAction::PrintVersion { verbose: true })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-V", "-V"])),
            Ok(CliAction::PrintVersion { verbose: true })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-V", "-c", "print(1)"])),
            Ok(CliAction::PrintVersion { verbose: false })
        );
    }

    #[test]
    fn cli_double_dash_and_stdin() {
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "--", "-weird.py", "--arg"])),
            Ok(CliAction::RunFile {
                path: "-weird.py".into(),
                args: argv(&["--arg"]),
            })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "-", "a", "--b"])),
            Ok(CliAction::RunStdin {
                args: argv(&["a", "--b"]),
            })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "--", "-", "a"])),
            Ok(CliAction::RunStdin { args: argv(&["a"]) })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "--"])),
            Ok(CliAction::Repl)
        );
    }

    #[test]
    fn cli_script_stops_option_parsing_and_preserves_emit_mir() {
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "script.py", "-c", "foo", "-V"])),
            Ok(CliAction::RunFile {
                path: "script.py".into(),
                args: argv(&["-c", "foo", "-V"]),
            })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "--emit", "mir", "a.py"])),
            Ok(CliAction::EmitMir {
                path: "a.py".into()
            })
        );
        assert_eq!(
            parse_cli_args(&argv(&["pyrust", "--emit=mir", "a.py"])),
            Ok(CliAction::EmitMir {
                path: "a.py".into()
            })
        );
    }

    #[test]
    fn cli_usage_errors_exit_with_code_2() {
        let missing_c = parse_cli_args(&argv(&["pyrust", "-c"])).unwrap_err();
        assert_eq!(missing_c.exit_code, 2);
        assert!(missing_c.message.contains("-c"));

        let missing_m = parse_cli_args(&argv(&["pyrust", "-m"])).unwrap_err();
        assert_eq!(missing_m.exit_code, 2);
        assert!(missing_m.message.contains("-m"));

        let unknown_short = parse_cli_args(&argv(&["pyrust", "-Z"])).unwrap_err();
        assert_eq!(unknown_short.exit_code, 2);
        assert!(unknown_short.message.contains("Unknown option: -Z"));

        let unknown_long = parse_cli_args(&argv(&["pyrust", "--bogus"])).unwrap_err();
        assert_eq!(unknown_long.exit_code, 2);
        assert!(unknown_long.message.contains("Unknown option: --bogus"));

        let version_then_unknown = parse_cli_args(&argv(&["pyrust", "-V", "--bogus"])).unwrap_err();
        assert_eq!(version_then_unknown.exit_code, 2);
    }
}
