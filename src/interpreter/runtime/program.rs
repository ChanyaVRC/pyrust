impl Interpreter {
    pub fn with_script_dir(dir: PathBuf) -> Self {
        let mut interp = Self::default();
        interp.script_dir = Some(dir);
        interp
    }

    pub fn exec_program(&mut self, program: &[Stmt], repl_mode: bool) -> Result<()> {
        if let Some(result) = self.try_exec_vm_script(program, repl_mode) {
            return result;
        }
        Err(PyError::Runtime("compilation failed".to_string()))
    }

    fn try_exec_vm_script(&mut self, program: &[Stmt], repl_mode: bool) -> Option<Result<()>> {
        // Use an empty local_index so all module-level assignments go through
        // StoreGlobal → assign_name → env.  This ensures that function
        // definitions are immediately visible by name for recursive calls.
        let local_index: Rc<HashMap<String, usize>> = Rc::new(HashMap::new());
        let code = Rc::new(crate::compiler::compile_script(program, Rc::clone(&local_index), repl_mode)?);
        let num_regs = code.num_regs as usize;
        let mut regs: Vec<Option<Value>> = vec![None; num_regs];
        self.call_depth += 1;
        let vm_result = self.run_bytecode(&code, &mut regs);
        self.call_depth -= 1;
        Some(vm_result.map(|_| ()))
    }

    fn apply_decorators(&mut self, mut value: Value, decorators: &[Expr]) -> Result<Value> {
        for deco_expr in decorators.iter().rev() {
            let deco = self.eval_expr(deco_expr)?;
            value = self.call_function_expanded(
                deco,
                &[ExpandedCallArg {
                    name: None,
                    value,
                }],
            )?;
        }
        Ok(value)
    }

}
