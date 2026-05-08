mod ast;
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

fn run_file(path: &str) -> Result<()> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| PyError::Runtime(format!("failed to read '{path}': {e}")))?;
    let program = parse_source(&src)?;
    let script_dir = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut interpreter = Interpreter::with_script_dir(script_dir);
    interpreter.exec_program(&program, false)
}

fn run_repl() -> Result<()> {
    println!("PyRust 0.2 (Python-like subset). Type 'exit' or 'quit' to leave.");

    let mut interpreter = Interpreter::default();
    let stdin = io::stdin();

    loop {
        print!(">>> ");
        io::stdout()
            .flush()
            .map_err(|e| PyError::Runtime(format!("stdout flush failed: {e}")))?;

        let mut line = String::new();
        let n = stdin
            .read_line(&mut line)
            .map_err(|e| PyError::Runtime(format!("stdin read failed: {e}")))?;
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
        match parse_source(&one_line).and_then(|program| interpreter.exec_program(&program, true)) {
            Ok(()) => {}
            Err(e) => eprintln!("{e}"),
        }
    }

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
