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
        // Cap script-level fastlocals so the register array stays small.
        // Scripts with more names than this fall back to all-env mode where names
        // live in a HashMap rather than a Vec<Option<Value>>.
        const MAX_SCRIPT_LOCALS: usize = 200;
        if local_names.len() > MAX_SCRIPT_LOCALS {
            // Too many locals — fall back to all-env mode.
            let local_index: Rc<HashMap<String, crate::bytecode::Reg>> = Rc::new(HashMap::new());
            return self.try_exec_vm_script_with_index(program, local_index, repl_mode);
        }
        let local_index: Rc<HashMap<String, crate::bytecode::Reg>> = Rc::new(
            (0u32..).zip(local_names.iter())
                .map(|(i, n)| (n.clone(), i))
                .collect(),
        );
        self.try_exec_vm_script_with_index(program, local_index, repl_mode)
    }

    fn try_exec_vm_script_with_index(
        &mut self,
        program: &[Stmt],
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        repl_mode: bool,
    ) -> Option<Result<()>> {
        let code = match crate::compiler::compile_script(program, Rc::clone(&local_index), repl_mode) {
            Ok(c) => Rc::new(crate::optimizer::optimize(c)),
            Err(e) => return Some(Err(e)),
        };
        let num_regs = code.num_regs as usize;
        let mut regs: Vec<Option<Value>> = vec![None; num_regs];
        let _depth_guard = CallDepthGuard::enter();
        let vm_result = self.run_bytecode(&code, &mut regs);
        // Write fastlocal registers back to the module env so that imported
        // modules and post-run inspection can find all names.
        for (name, &idx) in local_index.iter() {
            if let Some(val) = regs[idx as usize].take() {
                self.assign_name(name.clone(), val);
            }
        }
        Some(vm_result.map(|_| ()))
    }

}
