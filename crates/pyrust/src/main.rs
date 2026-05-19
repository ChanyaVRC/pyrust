mod ast;
mod builtin_modules;
mod builtin_registry;
mod bytecode;
mod compiler;
mod error;
mod interpreter;
mod lexer;
mod optimizer;
mod parser;
mod token;
mod value;

use error::{PyError, Result};
use interpreter::Interpreter;
use lexer::Lexer;
use parser::Parser;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

fn parse_source(src: &str) -> Result<Vec<ast::Stmt>> {
    let tokens = Lexer::new(src)?.into_tokens();
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

// 256 MB stack so that deep Python recursion (up to RecursionError limit of
// 1000 calls) never overflows the OS default (1 MB on Windows), even in
// debug builds where Rust stack frames can be 80–100 KB each.
const INTERPRETER_STACK_SIZE: usize = 256 * 1024 * 1024;

fn run_file(path: &str) -> Result<()> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| PyError::Runtime(format!("failed to read '{path}': {e}")))?;
    let program = parse_source(&src)?;
    let script_dir = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    // Marshal errors as strings so the Result crosses the thread boundary
    // (Value contains Rc which is not Send).
    let err_str: Option<String> = std::thread::Builder::new()
        .stack_size(INTERPRETER_STACK_SIZE)
        .spawn(move || {
            let mut interp = Interpreter::with_script_dir(script_dir);
            interp
                .exec_program(&program, false)
                .err()
                .map(|e| e.to_string())
        })
        .expect("failed to spawn interpreter thread")
        .join()
        .expect("interpreter thread panicked");

    if let Some(msg) = err_str {
        eprintln!("{msg}");
        std::process::exit(1);
    }
    Ok(())
}

fn last_line_is_indented(buf: &str) -> bool {
    buf.lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| l.starts_with([' ', '\t']))
        .unwrap_or(false)
}

fn is_incomplete(err: &PyError) -> bool {
    match err {
        // "found Eof" — input cut off mid-expression
        // "expected Indent, found" — block header (e.g. "for x:") with no body yet;
        //   the trailing \n produces an extra Newline token instead of Eof
        PyError::Parse(msg) | PyError::Lex(msg) => {
            msg.contains("found Eof") || msg.contains("expected Indent, found")
        }
        _ => false,
    }
}

fn run_repl() -> Result<()> {
    println!("PyRust 0.2 (Python-like subset). Type 'exit' or 'quit' to leave.");

    let mut rl = DefaultEditor::new().map_err(|e| PyError::Runtime(e.to_string()))?;

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
                    match parse_source(&src).and_then(|p| interpreter.exec_program(&p, true)) {
                        Ok(()) => {}
                        Err(e) => eprintln!("{e}"),
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
                        eprintln!("{e}");
                    }
                }
            }
        })
        .expect("failed to spawn REPL thread")
        .join()
        .expect("REPL thread panicked");

    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = if args.len() > 1 {
        run_file(&args[1])
    } else {
        run_repl()
    };

    if let Err(e) = result {
        eprintln!("{e}");
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
        assert!(is_incomplete(&PyError::Parse(
            "expected X, found Eof".into()
        )));
    }

    #[test]
    fn expected_indent_parse_error_is_incomplete() {
        // produced when a block header line ends with '\n' and the lexer emits
        // an extra Newline token instead of Eof
        assert!(is_incomplete(&PyError::Parse(
            "expected Indent, found Some(Newline)".into()
        )));
    }

    #[test]
    fn eof_lex_error_is_incomplete() {
        assert!(is_incomplete(&PyError::Lex("found Eof".into())));
    }

    #[test]
    fn unrelated_parse_error_is_not_incomplete() {
        assert!(!is_incomplete(&PyError::Parse(
            "unexpected token in expression: Some(Plus)".into()
        )));
    }

    #[test]
    fn runtime_error_is_not_incomplete() {
        assert!(!is_incomplete(&PyError::Runtime("name error".into())));
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
}
