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
        // Build fastlocal registers for module-level names that are NOT captured
        // by nested functions (those become cell vars and use StoreGlobal/env).
        // This allows tight loops over plain variables to avoid HashMap overhead.
        let empty: HashSet<String> = HashSet::new();
        let local_names =
            crate::interpreter::collect_local_names(&[], program, &empty, &empty);
        // Reg type is u8, so at most 255 slots. Keep a safety margin so temp
        // registers don't collide with locals.
        const MAX_SCRIPT_LOCALS: usize = 200;
        if local_names.len() > MAX_SCRIPT_LOCALS {
            // Too many locals — fall back to all-env mode.
            let local_index: Rc<HashMap<String, usize>> = Rc::new(HashMap::new());
            return self.try_exec_vm_script_with_index(program, local_index, repl_mode);
        }
        let local_index: Rc<HashMap<String, usize>> = Rc::new(
            local_names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), i))
                .collect(),
        );
        self.try_exec_vm_script_with_index(program, local_index, repl_mode)
    }

    fn try_exec_vm_script_with_index(
        &mut self,
        program: &[Stmt],
        local_index: Rc<HashMap<String, usize>>,
        repl_mode: bool,
    ) -> Option<Result<()>> {
        let code = Rc::new(crate::compiler::compile_script(
            program,
            Rc::clone(&local_index),
            repl_mode,
        )?);
        let num_regs = code.num_regs as usize;
        let mut regs: Vec<Option<Value>> = vec![None; num_regs];
        self.call_depth += 1;
        let vm_result = self.run_bytecode(&code, &mut regs);
        self.call_depth -= 1;
        // Write fastlocal registers back to the module env so that imported
        // modules and post-run inspection can find all names.
        for (name, &idx) in local_index.iter() {
            if let Some(val) = regs[idx].take() {
                self.assign_name(name.clone(), val);
            }
        }
        Some(vm_result.map(|_| ()))
    }

}
