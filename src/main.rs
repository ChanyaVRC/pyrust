mod ast;
mod bytecode;
mod compiler;
mod error;
mod interpreter;
mod lexer;
mod parser;
mod token;
mod value;

use std::io::{self, Write};

use error::{PyError, Result};
use interpreter::Interpreter;
use lexer::Lexer;
use parser::Parser;

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

fn run_repl() -> Result<()> {
    println!("PyRust 0.2 (Python-like subset). Type 'exit' or 'quit' to leave.");

    let stdin = io::stdin();
    std::thread::Builder::new()
        .stack_size(INTERPRETER_STACK_SIZE)
        .spawn(move || {
            let mut interpreter = Interpreter::default();
            loop {
                print!(">>> ");
                io::stdout().flush().ok();

                let mut line = String::new();
                let n = stdin.read_line(&mut line).unwrap_or(0);
                if n == 0 {
                    break;
                }

                let src = line.trim_end();
                if src.trim().is_empty() {
                    continue;
                }
                if src.trim() == "exit" || src.trim() == "quit" {
                    break;
                }

                let one_line = format!("{src}\n");
                match parse_source(&one_line).and_then(|p| interpreter.exec_program(&p, true)) {
                    Ok(()) => {}
                    Err(e) => eprintln!("{e}"),
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
