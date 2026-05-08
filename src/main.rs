mod ast;
mod error;
mod interpreter;
mod lexer;
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
        return Err(PyError::Runtime(msg));
    }
    Ok(())
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
                    rl.add_history_entry(src.trim_end()).ok();
                    match parse_source(&src).and_then(|p| interpreter.exec_program(&p, true)) {
                        Ok(()) => {}
                        Err(e) => eprintln!("{e}"),
                    }
                    continue;
                }

                if trimmed.is_empty() {
                    continue;
                }

                buf.push_str(&line);
                buf.push('\n');

                // Lines ending with ':' always need a body — skip parse attempt
                if trimmed.ends_with(':') {
                    continue;
                }

                // Try to parse and execute; stay in continuation if incomplete
                match parse_source(&buf) {
                    Ok(program) => {
                        rl.add_history_entry(buf.trim_end()).ok();
                        buf.clear();
                        if let Err(e) = interpreter.exec_program(&program, true) {
                            eprintln!("{e}");
                        }
                    }
                    Err(e) if is_incomplete(&e) => {
                        // Need more input — keep buf and show "..." next iteration
                    }
                    Err(e) => {
                        rl.add_history_entry(buf.trim_end()).ok();
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
