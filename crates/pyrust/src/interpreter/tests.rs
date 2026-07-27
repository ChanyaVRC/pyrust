#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    // `range_len` is no longer used in this module's non-test code (it
    // moved to `builtin_modules::builtins::len`); pull it in for the
    // legacy unit test below.
    use crate::value::range_len;

    fn run_program(src: &str) -> Interpreter {
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();
        interpreter.exec_program(&program, false).unwrap();
        interpreter
    }

    // Test groups share the helpers and imports in this module.
    include!("tests/scopes_and_syntax.rs");
    include!("tests/classes_exceptions_imports.rs");
    include!("tests/calls_and_loops.rs");
    include!("tests/vm_execution.rs");
    include!("tests/runtime_regressions.rs");
    include!("tests/numeric_and_errors.rs");
    include!("tests/protocols_and_paths.rs");
    include!("tests/library_modules.rs");
    include!("tests/declaration_syntax.rs");
}
