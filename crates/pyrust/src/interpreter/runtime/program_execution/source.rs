impl Interpreter {
    /// Parse a Python source string into a statement list plus the per-statement
    /// 1-based line-number table, so the compiler can thread accurate line
    /// numbers into the bytecode for `exec`/`eval`/`compile`'d source.  Without
    /// this, errors raised *inside* exec'd code report wrong internal line
    /// numbers (issue #2245): the original path discarded the lexer's physical
    /// line table via `into_tokens()`.  Converts lexer/parse errors into
    /// `SyntaxError` (or its `IndentationError` subclass for indentation
    /// failures) exceptions.
    pub(crate) fn parse_source_to_stmts_with_linenos(
        source: &str,
    ) -> Result<(Vec<crate::ast::Stmt>, Vec<u32>)> {
        let (tokens, line_nos, cols, cols_end) = crate::lexer::Lexer::new(source)
            .map_err(lex_parse_to_exc)?
            .into_tokens_with_pos();
        let mut parser = crate::parser::Parser::new_with_pos(tokens, line_nos, cols, cols_end);
        parser
            .parse_program_with_linenos()
            .map_err(lex_parse_to_exc)
    }

    /// Parse a Python source string into statements + line numbers while
    /// emitting any lexer-collected `SyntaxWarning`s (e.g. invalid escape
    /// sequences) through this interpreter's live `warnings` system.
    pub(crate) fn parse_source_to_stmts_with_warnings(
        &mut self,
        source: &str,
        filename: &str,
    ) -> Result<(Vec<crate::ast::Stmt>, Vec<u32>)> {
        let (tokens, line_nos, cols, cols_end, warnings) = crate::lexer::Lexer::new(source)
            .map_err(lex_parse_to_exc)?
            .into_tokens_with_pos_and_warnings();
        for (lineno, msg) in warnings {
            crate::interpreter::emit_syntax_warning_via_module(
                self,
                &msg,
                filename,
                lineno as i64,
            )?;
        }
        let mut parser = crate::parser::Parser::new_with_pos(tokens, line_nos, cols, cols_end);
        parser
            .parse_program_with_linenos()
            .map_err(lex_parse_to_exc)
    }

    /// Parse a Python source string as a single interactive statement for
    /// `compile(..., 'single')`, emitting lexer `SyntaxWarning`s through the
    /// live `warnings` system.
    pub(crate) fn parse_single_source_to_stmts_with_warnings(
        &mut self,
        source: &str,
        filename: &str,
    ) -> Result<(Vec<crate::ast::Stmt>, Vec<u32>)> {
        let (tokens, line_nos, cols, cols_end, warnings) = crate::lexer::Lexer::new(source)
            .map_err(lex_parse_to_exc)?
            .into_tokens_with_pos_and_warnings();
        for (lineno, msg) in warnings {
            crate::interpreter::emit_syntax_warning_via_module(
                self,
                &msg,
                filename,
                lineno as i64,
            )?;
        }
        let mut parser = crate::parser::Parser::new_with_pos(tokens, line_nos, cols, cols_end);
        let ends_with_newline = source.ends_with('\n') || source.ends_with('\r');
        parser
            .parse_single_with_linenos(ends_with_newline)
            .map_err(lex_parse_to_exc)
    }

    /// Parse a Python source string as a single interactive statement for
    /// `compile(..., 'single')`.
    #[allow(dead_code)]
    pub(crate) fn parse_single_source_to_stmts_with_linenos(
        source: &str,
    ) -> Result<(Vec<crate::ast::Stmt>, Vec<u32>)> {
        let (tokens, line_nos, cols, cols_end) = crate::lexer::Lexer::new(source)
            .map_err(lex_parse_to_exc)?
            .into_tokens_with_pos();
        let mut parser = crate::parser::Parser::new_with_pos(tokens, line_nos, cols, cols_end);
        let ends_with_newline = source.ends_with('\n') || source.ends_with('\r');
        parser
            .parse_single_with_linenos(ends_with_newline)
            .map_err(lex_parse_to_exc)
    }

    /// Execute a source string as statements, optionally in an explicit
    /// namespace.
    ///
    /// - `globals_dict`: when `None`, runs in the current interpreter's module
    ///   namespace (assignments become globals).  When `Some(dict)`, the dict
    ///   is used as both the globals and locals namespace; assignments write
    ///   back to the dict.
    /// - `locals_dict`: when `Some(dict)` (and `globals_dict` is also `Some`),
    ///   name lookups check this dict first; assignments go to `locals_dict`.
    ///   Matches CPython's exec(code, globals, locals) semantics.
    pub(crate) fn exec_source(
        &mut self,
        source: &str,
        globals_dict: Option<Value>,
        locals_dict: Option<Value>,
    ) -> Result<()> {
        self.exec_source_with_filename(source, "<string>", globals_dict, locals_dict)
    }

    /// Execute a source string as statements with an explicit code-object
    /// `filename` (e.g. `"<string>"` for `exec()`, or `"<frozen asyncio>"` for
    /// bundled stdlib sources — issue #2993).
    pub(crate) fn exec_source_with_filename(
        &mut self,
        source: &str,
        filename: &str,
        globals_dict: Option<Value>,
        locals_dict: Option<Value>,
    ) -> Result<()> {
        let _int_max_str_digits_guard = IntMaxStrDigitsExecutionGuard::enter(self);
        let (program, linenos) = self.parse_source_to_stmts_with_warnings(source, filename)?;
        match globals_dict {
            None => {
                // No explicit namespace: compile and run in the current module
                // scope, but do NOT go through try_exec_vm_script_with_index —
                // that path converts any raised exception into PyError::Runtime
                // (the traceback-formatted string) which makes the exception
                // uncatchable by type.  exec() must propagate the raw exception
                // so callers can catch ZeroDivisionError, NameError, etc.
                use std::collections::HashSet;
                let empty: HashSet<String> = HashSet::new();
                let global_names = crate::interpreter::collect_global_names(&program);
                let local_names =
                    crate::interpreter::collect_local_names(&[], &program, &global_names, &empty);
                const MAX_SCRIPT_LOCALS: usize = 200;
                let local_index: Rc<HashMap<String, crate::bytecode::Reg>> =
                    if local_names.len() <= MAX_SCRIPT_LOCALS {
                        Rc::new(
                            (0u32..)
                                .zip(local_names.iter())
                                .map(|(i, n)| (n.clone(), i))
                                .collect(),
                        )
                    } else {
                        Rc::new(HashMap::new())
                    };
                // Thread the lexer line table into the bytecode (issue #2245)
                // so errors inside the exec'd source report correct internal
                // line numbers.
                let code = {
                    let c = crate::compiler::compile_script_with_options(
                        &program,
                        Rc::clone(&local_index),
                        false,
                        &linenos,
                        filename,
                        crate::compiler::CompileOptions {
                            optimize: self.compile_optimize_level(),
                            allow_top_level_await: false,
                        },
                    )?;
                    Rc::new(crate::optimizer::optimize(c))
                };
                let num_regs = code.num_regs as usize;
                let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
                let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView::script_frame(
                    regs_ptr,
                    regs_len,
                    Rc::clone(&local_index),
                    Rc::clone(&self.env),
                ));
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let namespace_mirror_guard =
                    self.register_script_namespace_mirror(regs_ptr, regs_len, &local_index);
                let vm_result = self.run_bytecode(&code, regs_slice);
                drop(namespace_mirror_guard);
                self.detach_vm_frame_view();
                record_exec_string_frame(self, &vm_result, &code.filename);
                // Write fastlocals back to module env so names are visible
                // after exec() returns, matching top-level assignment semantics.
                self.write_back_script_locals(&local_index, &mut regs);
                drop(regs);
                self.finish_pop_vm_frame_view();
                vm_result.map(|_| ())
            }
            Some(gdict) => {
                // Explicit globals dict: seed a fresh env from the dict, run,
                // then write all new/changed names back to the dict.
                self.exec_in_dict_env(&program, &linenos, filename, gdict, locals_dict)
            }
        }
    }

    /// Evaluate a source string as a single expression.
    ///
    /// Same namespace semantics as `exec_source`.  Returns the expression's
    /// value.
    pub(crate) fn eval_source(
        &mut self,
        source: &str,
        globals_dict: Option<Value>,
        locals_dict: Option<Value>,
    ) -> Result<Value> {
        let _int_max_str_digits_guard = IntMaxStrDigitsExecutionGuard::enter(self);
        // Strip leading and trailing whitespace: CPython's `eval()` strips both.
        // `eval("  1 + 2  ")` and `eval("1 + 2\n")` both work in CPython.
        let trimmed = source.trim();
        let (program, linenos) = self.parse_source_to_stmts_with_warnings(trimmed, "<string>")?;
        let local_index: Rc<HashMap<String, crate::bytecode::Reg>> = Rc::new(HashMap::new());
        let code = {
            let c = crate::compiler::compile_eval_expr_with_options(
                &program,
                Rc::clone(&local_index),
                &linenos,
                "<string>",
                crate::compiler::CompileOptions {
                    optimize: self.compile_optimize_level(),
                    allow_top_level_await: false,
                },
            )?;
            Rc::new(crate::optimizer::optimize(c))
        };
        match globals_dict {
            None => {
                // Run in current module namespace.
                self.run_eval_code_in_module(&code, local_index)
            }
            Some(gdict) => self.eval_in_dict_env(&code, local_index, gdict, locals_dict),
        }
    }

    /// Run a compiled eval-mode code object in the current module namespace.
    /// Pushes a Script VmFrameView so `globals()`/`locals()` work inside the
    /// evaluated expression.
    fn run_eval_code_in_module(
        &mut self,
        code: &Rc<crate::bytecode::FnCode>,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
    ) -> Result<Value> {
        let num_regs = code.num_regs as usize;
        let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
        let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
        let regs_len = regs.len();
        self.vm_frame_views.push(VmFrameView::script_frame(
            regs_ptr,
            regs_len,
            Rc::clone(&local_index),
            Rc::clone(&self.env),
        ));
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let namespace_mirror_guard =
            self.register_script_namespace_mirror(regs_ptr, regs_len, &local_index);
        let vm_result = self.run_bytecode(code, regs_slice);
        drop(namespace_mirror_guard);
        self.detach_vm_frame_view();
        drop(regs);
        self.finish_pop_vm_frame_view();
        record_exec_string_frame(self, &vm_result, &code.filename);
        vm_result
    }
}
