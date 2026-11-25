// `_symtable` module — 100% native Rust implementation of Python 3.12's
// `_symtable` compiler symbol-table analyzer (issue #3249).
//
// Parses Python source via PyRust's full Python 3.12 AST parser (`ast.parse`),
// performs a two-pass lexical scope analysis matching CPython 3.12 (including
// PEP 709 inlined sync comprehensions, closures, cell/free propagation,
// private class name mangling, `global`, `nonlocal`, annotations, and
// PEP 695 type parameters), and returns native `symtable` entry trees.
//
// Reference: <https://docs.python.org/3.12/library/symtable.html>

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, Value, ValueKind};

// ── CPython 3.12 `_symtable` bitflags & scope constants ──────────────────────

pub(crate) const DEF_GLOBAL: i64 = 1;
pub(crate) const DEF_LOCAL: i64 = 2;
pub(crate) const DEF_PARAM: i64 = 4;
pub(crate) const DEF_NONLOCAL: i64 = 8;
pub(crate) const USE: i64 = 16;
pub(crate) const DEF_FREE: i64 = 32;
pub(crate) const DEF_FREE_CLASS: i64 = 64;
pub(crate) const DEF_IMPORT: i64 = 128;
pub(crate) const DEF_ANNOT: i64 = 256;
pub(crate) const DEF_BOUND: i64 = DEF_LOCAL | DEF_PARAM | DEF_IMPORT; // 134

pub(crate) const SCOPE_OFF: i64 = 12;
pub(crate) const SCOPE_MASK: i64 = 15;

pub(crate) const LOCAL: i64 = 1;
pub(crate) const GLOBAL_EXPLICIT: i64 = 2;
pub(crate) const GLOBAL_IMPLICIT: i64 = 3;
pub(crate) const FREE: i64 = 4;
pub(crate) const CELL: i64 = 5;

pub(crate) const TYPE_FUNCTION: i64 = 0;
pub(crate) const TYPE_CLASS: i64 = 1;
pub(crate) const TYPE_MODULE: i64 = 2;
pub(crate) const TYPE_ANNOTATION: i64 = 3;
pub(crate) const TYPE_TYPE_VAR_BOUND: i64 = 4;
pub(crate) const TYPE_TYPE_ALIAS: i64 = 5;
pub(crate) const TYPE_TYPE_PARAM: i64 = 6;

thread_local! {
    static NEXT_BLOCK_ID: Cell<i64> = const { Cell::new(1) };
}

fn alloc_block_id() -> i64 {
    NEXT_BLOCK_ID.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1));
        id
    })
}

/// Analyzed immutable symbol-table block shared between `_symtable` and `symtable`.
#[derive(Debug, Clone)]
pub(crate) struct RawSymTable {
    pub id: i64,
    pub block_type: i64,
    pub name: String,
    pub lineno: i64,
    pub optimized: bool,
    pub nested: bool,
    pub symbols: IndexMap<String, i64>,
    pub varnames: Vec<String>,
    pub children: Vec<Rc<RawSymTable>>,
}

impl RawSymTable {
    pub(crate) fn type_name(&self) -> &'static str {
        match self.block_type {
            TYPE_MODULE => "module",
            TYPE_FUNCTION => "function",
            TYPE_CLASS => "class",
            TYPE_ANNOTATION => "annotation",
            TYPE_TYPE_VAR_BOUND => "TypeVar bound",
            TYPE_TYPE_ALIAS => "type alias",
            TYPE_TYPE_PARAM => "type parameter",
            _ => "unknown",
        }
    }
}

/// Mutable working block used during Pass 1 & Pass 2 analysis.
struct WorkBlock {
    id: i64,
    block_type: i64,
    name: String,
    lineno: i64,
    optimized: bool,
    nested: bool,
    is_comprehension: bool,
    symbols: IndexMap<String, i64>,
    /// Symbols collected from PEP 709 inlined comprehensions, merged after the
    /// enclosing block's statements to match CPython 3.12's insertion order.
    inlined_comp_symbols: IndexMap<String, i64>,
    varnames: Vec<String>,
    children: Vec<WorkBlock>,
    /// Tracks whether `global` was declared in this block for each name.
    explicit_globals: HashSet<String>,
}

impl WorkBlock {
    fn new(
        block_type: i64,
        name: String,
        lineno: i64,
        optimized: bool,
        nested: bool,
        is_comprehension: bool,
    ) -> Self {
        Self {
            id: alloc_block_id(),
            block_type,
            name,
            lineno,
            optimized,
            nested,
            is_comprehension,
            symbols: IndexMap::new(),
            inlined_comp_symbols: IndexMap::new(),
            varnames: Vec::new(),
            children: Vec::new(),
            explicit_globals: HashSet::new(),
        }
    }

    fn add_flag(&mut self, name: &str, flag: i64) {
        let entry = self.symbols.entry(name.to_string()).or_insert(0);
        *entry |= flag;
    }

    fn flush_inlined_comp_symbols(&mut self) {
        let deferred = std::mem::take(&mut self.inlined_comp_symbols);
        for (name, flag) in deferred {
            self.add_flag(&name, flag);
        }
    }

    fn into_raw(self) -> Rc<RawSymTable> {
        let children = self.children.into_iter().map(WorkBlock::into_raw).collect();
        Rc::new(RawSymTable {
            id: self.id,
            block_type: self.block_type,
            name: self.name,
            lineno: self.lineno,
            optimized: self.optimized,
            nested: self.nested,
            symbols: self.symbols,
            varnames: self.varnames,
            children,
        })
    }
}

/// CPython private name mangling inside a class scope (`__x` -> `_Cls__x`).
fn mangle_name(class_name: Option<&str>, ident: &str) -> String {
    let Some(cls) = class_name else {
        return ident.to_string();
    };
    if !ident.starts_with("__") || ident.ends_with("__") || ident.contains('.') {
        return ident.to_string();
    }
    let stripped = cls.trim_start_matches('_');
    if stripped.is_empty() {
        return ident.to_string();
    }
    format!("_{stripped}{ident}")
}

// ── AST node inspection helpers ──────────────────────────────────────────────

fn ast_class_name(val: &Value) -> Option<String> {
    match val.kind() {
        ValueKind::PyInstance(inst) => Some(inst.borrow().class.borrow().name.clone()),
        _ => None,
    }
}

fn ast_field(val: &Value, field: &str) -> Option<Value> {
    match val.kind() {
        ValueKind::PyInstance(inst) => inst.borrow().attrs.get(field).cloned(),
        _ => None,
    }
}

fn ast_str(val: &Value, field: &str) -> Option<String> {
    ast_field(val, field).and_then(|v| v.as_str().map(str::to_owned))
}

fn ast_int(val: &Value, field: &str) -> i64 {
    ast_field(val, field)
        .and_then(|v| match v.kind() {
            ValueKind::Int(n) => Some(n),
            _ => None,
        })
        .unwrap_or(0)
}

fn ast_bool(val: &Value, field: &str) -> bool {
    ast_field(val, field)
        .map(|v| match v.kind() {
            ValueKind::Bool(b) => b,
            ValueKind::Int(n) => n != 0,
            _ => false,
        })
        .unwrap_or(false)
}

fn ast_list(val: &Value, field: &str) -> Vec<Value> {
    ast_field(val, field)
        .map(|v| match v.kind() {
            ValueKind::List(items) => items.clone(),
            ValueKind::Tuple(items) => items.to_vec(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

// ── Pass 1: AST Walker ───────────────────────────────────────────────────────

struct AstAnalyzer {
    /// Stack of active lexical blocks; index 0 is always the module root.
    stack: Vec<WorkBlock>,
    /// Current enclosing class name for private name mangling.
    class_stack: Vec<String>,
}

impl AstAnalyzer {
    fn new() -> Self {
        let root = WorkBlock::new(TYPE_MODULE, "top".to_string(), 0, false, false, false);
        Self {
            stack: vec![root],
            class_stack: Vec::new(),
        }
    }

    fn current_class(&self) -> Option<&str> {
        self.class_stack.last().map(String::as_str)
    }

    fn mangle(&self, ident: &str) -> String {
        mangle_name(self.current_class(), ident)
    }

    fn current_mut(&mut self) -> &mut WorkBlock {
        self.stack.last_mut().expect("symtable stack non-empty")
    }

    fn is_nested_child(&self) -> bool {
        let cur = self.stack.last().expect("symtable stack non-empty");
        cur.nested || cur.block_type == TYPE_FUNCTION
    }

    fn add_def(&mut self, ident: &str, flag: i64) {
        let mangled = self.mangle(ident);
        self.current_mut().add_flag(&mangled, flag);
    }

    fn add_global(&mut self, ident: &str) {
        let mangled = self.mangle(ident);
        self.current_mut().add_flag(&mangled, DEF_GLOBAL);
        self.current_mut().explicit_globals.insert(mangled.clone());
        // CPython also marks the symbol in the top-level module block.
        if self.stack.len() > 1 {
            self.stack[0].add_flag(&mangled, DEF_GLOBAL);
        }
    }

    fn add_walrus_target(&mut self, ident: &str) {
        let mangled = self.mangle(ident);
        // PEP 572: `:=` binds in the nearest enclosing non-comprehension scope.
        let target_idx = self
            .stack
            .iter()
            .rposition(|b| !b.is_comprehension)
            .unwrap_or(0);
        self.stack[target_idx].add_flag(&mangled, DEF_LOCAL);
    }

    fn push_block(
        &mut self,
        block_type: i64,
        name: String,
        lineno: i64,
        optimized: bool,
        is_comprehension: bool,
        _class_name: Option<String>,
    ) {
        let nested = self.is_nested_child();
        let blk = WorkBlock::new(
            block_type,
            name,
            lineno,
            optimized,
            nested,
            is_comprehension,
        );
        self.stack.push(blk);
    }

    fn pop_block(&mut self) {
        let mut finished = self.stack.pop().expect("symtable pop");
        finished.flush_inlined_comp_symbols();
        self.current_mut().children.push(finished);
    }

    fn visit_module(&mut self, node: &Value) {
        let Some(kind) = ast_class_name(node) else {
            return;
        };
        match kind.as_str() {
            "Module" | "Interactive" => {
                for stmt in ast_list(node, "body") {
                    self.visit_stmt(&stmt);
                }
            }
            "Expression" => {
                if let Some(body) = ast_field(node, "body") {
                    self.visit_expr(&body);
                }
            }
            _ => {}
        }
    }

    fn visit_stmts(&mut self, stmts: &[Value]) {
        for stmt in stmts {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Value) {
        let Some(kind) = ast_class_name(stmt) else {
            return;
        };
        match kind.as_str() {
            "FunctionDef" | "AsyncFunctionDef" => {
                let fn_name = ast_str(stmt, "name").unwrap_or_default();
                let lineno = ast_int(stmt, "lineno");
                self.add_def(&fn_name, DEF_LOCAL);
                for dec in ast_list(stmt, "decorator_list") {
                    self.visit_expr(&dec);
                }
                let args_node = ast_field(stmt, "args");
                let type_params = ast_list(stmt, "type_params");
                // Visit defaults in enclosing scope.
                if let Some(ref args) = args_node {
                    self.visit_arg_defaults(args);
                }
                if type_params.is_empty() {
                    if let Some(ref args) = args_node {
                        self.visit_arg_annotations(args);
                    }
                    if let Some(ret) = ast_field(stmt, "returns")
                        && !ret.is_none()
                    {
                        self.visit_expr(&ret);
                    }
                } else {
                    self.visit_type_params(&type_params, &fn_name, lineno, |this| {
                        if let Some(ref args) = args_node {
                            this.visit_arg_annotations(args);
                        }
                        if let Some(ret) = ast_field(stmt, "returns")
                            && !ret.is_none()
                        {
                            this.visit_expr(&ret);
                        }
                    });
                }
                self.push_block(TYPE_FUNCTION, fn_name, lineno, true, false, None);
                if let Some(ref args) = args_node {
                    self.visit_arg_names(args);
                }
                let body = ast_list(stmt, "body");
                self.visit_stmts(&body);
                self.pop_block();
            }
            "ClassDef" => {
                let cls_name = ast_str(stmt, "name").unwrap_or_default();
                let lineno = ast_int(stmt, "lineno");
                self.add_def(&cls_name, DEF_LOCAL);
                for dec in ast_list(stmt, "decorator_list") {
                    self.visit_expr(&dec);
                }
                let type_params = ast_list(stmt, "type_params");
                if type_params.is_empty() {
                    for base in ast_list(stmt, "bases") {
                        self.visit_expr(&base);
                    }
                    for kw in ast_list(stmt, "keywords") {
                        if let Some(v) = ast_field(&kw, "value") {
                            self.visit_expr(&v);
                        }
                    }
                } else {
                    self.visit_type_params(&type_params, &cls_name, lineno, |this| {
                        for base in ast_list(stmt, "bases") {
                            this.visit_expr(&base);
                        }
                        for kw in ast_list(stmt, "keywords") {
                            if let Some(v) = ast_field(&kw, "value") {
                                this.visit_expr(&v);
                            }
                        }
                    });
                }
                self.push_block(
                    TYPE_CLASS,
                    cls_name.clone(),
                    lineno,
                    false,
                    false,
                    Some(cls_name.clone()),
                );
                self.class_stack.push(cls_name);
                let body = ast_list(stmt, "body");
                self.visit_stmts(&body);
                self.class_stack.pop();
                self.pop_block();
            }
            "Return" => {
                if let Some(val) = ast_field(stmt, "value")
                    && !val.is_none()
                {
                    self.visit_expr(&val);
                }
            }
            "Delete" => {
                for target in ast_list(stmt, "targets") {
                    self.visit_expr(&target);
                }
            }
            "Assign" => {
                for target in ast_list(stmt, "targets") {
                    self.visit_expr(&target);
                }
                if let Some(val) = ast_field(stmt, "value") {
                    self.visit_expr(&val);
                }
            }
            "AugAssign" => {
                if let Some(target) = ast_field(stmt, "target") {
                    if ast_class_name(&target).as_deref() == Some("Name") {
                        if let Some(id) = ast_str(&target, "id") {
                            self.add_def(&id, DEF_LOCAL);
                        }
                    } else {
                        self.visit_expr(&target);
                    }
                }
                if let Some(val) = ast_field(stmt, "value") {
                    self.visit_expr(&val);
                }
            }
            "AnnAssign" => {
                let simple = ast_bool(stmt, "simple");
                if let Some(target) = ast_field(stmt, "target") {
                    if simple && ast_class_name(&target).as_deref() == Some("Name") {
                        if let Some(id) = ast_str(&target, "id") {
                            self.add_def(&id, DEF_ANNOT | DEF_LOCAL);
                        }
                    } else {
                        self.visit_expr(&target);
                    }
                }
                if let Some(ann) = ast_field(stmt, "annotation") {
                    self.visit_expr(&ann);
                }
                if let Some(val) = ast_field(stmt, "value")
                    && !val.is_none()
                {
                    self.visit_expr(&val);
                }
            }
            "TypeAlias" => {
                let lineno = ast_int(stmt, "lineno");
                let alias_name = ast_field(stmt, "name")
                    .and_then(|n| ast_str(&n, "id"))
                    .unwrap_or_default();
                self.add_def(&alias_name, DEF_LOCAL);
                let type_params = ast_list(stmt, "type_params");
                if !type_params.is_empty() {
                    self.visit_type_params(&type_params, &alias_name, lineno, |this| {
                        this.push_block(
                            TYPE_TYPE_ALIAS,
                            alias_name.clone(),
                            lineno,
                            true,
                            false,
                            None,
                        );
                        if let Some(val) = ast_field(stmt, "value") {
                            this.visit_expr(&val);
                        }
                        this.pop_block();
                    });
                } else {
                    self.push_block(TYPE_TYPE_ALIAS, alias_name, lineno, true, false, None);
                    if let Some(val) = ast_field(stmt, "value") {
                        self.visit_expr(&val);
                    }
                    self.pop_block();
                }
            }
            "For" | "AsyncFor" => {
                if let Some(target) = ast_field(stmt, "target") {
                    self.visit_expr(&target);
                }
                if let Some(iter) = ast_field(stmt, "iter") {
                    self.visit_expr(&iter);
                }
                self.visit_stmts(&ast_list(stmt, "body"));
                self.visit_stmts(&ast_list(stmt, "orelse"));
            }
            "While" | "If" => {
                if let Some(test) = ast_field(stmt, "test") {
                    self.visit_expr(&test);
                }
                self.visit_stmts(&ast_list(stmt, "body"));
                self.visit_stmts(&ast_list(stmt, "orelse"));
            }
            "With" | "AsyncWith" => {
                for item in ast_list(stmt, "items") {
                    if let Some(ctx) = ast_field(&item, "context_expr") {
                        self.visit_expr(&ctx);
                    }
                    if let Some(vars) = ast_field(&item, "optional_vars")
                        && !vars.is_none()
                    {
                        self.visit_expr(&vars);
                    }
                }
                self.visit_stmts(&ast_list(stmt, "body"));
            }
            "Match" => {
                if let Some(subject) = ast_field(stmt, "subject") {
                    self.visit_expr(&subject);
                }
                for case in ast_list(stmt, "cases") {
                    if let Some(pat) = ast_field(&case, "pattern") {
                        self.visit_pattern(&pat);
                    }
                    if let Some(guard) = ast_field(&case, "guard")
                        && !guard.is_none()
                    {
                        self.visit_expr(&guard);
                    }
                    self.visit_stmts(&ast_list(&case, "body"));
                }
            }
            "Raise" => {
                if let Some(exc) = ast_field(stmt, "exc")
                    && !exc.is_none()
                {
                    self.visit_expr(&exc);
                }
                if let Some(cause) = ast_field(stmt, "cause")
                    && !cause.is_none()
                {
                    self.visit_expr(&cause);
                }
            }
            "Try" | "TryStar" => {
                self.visit_stmts(&ast_list(stmt, "body"));
                for handler in ast_list(stmt, "handlers") {
                    if let Some(typ) = ast_field(&handler, "type")
                        && !typ.is_none()
                    {
                        self.visit_expr(&typ);
                    }
                    if let Some(name) = ast_str(&handler, "name") {
                        self.add_def(&name, DEF_LOCAL);
                    }
                    self.visit_stmts(&ast_list(&handler, "body"));
                }
                self.visit_stmts(&ast_list(stmt, "orelse"));
                self.visit_stmts(&ast_list(stmt, "finalbody"));
            }
            "Assert" => {
                if let Some(test) = ast_field(stmt, "test") {
                    self.visit_expr(&test);
                }
                if let Some(msg) = ast_field(stmt, "msg")
                    && !msg.is_none()
                {
                    self.visit_expr(&msg);
                }
            }
            "Import" => {
                for alias in ast_list(stmt, "names") {
                    let bound = ast_str(&alias, "asname").unwrap_or_else(|| {
                        let raw = ast_str(&alias, "name").unwrap_or_default();
                        raw.split('.').next().unwrap_or(&raw).to_string()
                    });
                    if !bound.is_empty() {
                        self.add_def(&bound, DEF_IMPORT);
                    }
                }
            }
            "ImportFrom" => {
                for alias in ast_list(stmt, "names") {
                    let raw = ast_str(&alias, "name").unwrap_or_default();
                    if raw == "*" {
                        continue;
                    }
                    let bound = ast_str(&alias, "asname").unwrap_or(raw);
                    if !bound.is_empty() {
                        self.add_def(&bound, DEF_IMPORT);
                    }
                }
            }
            "Global" => {
                for name_val in ast_list(stmt, "names") {
                    if let Some(name) = name_val.as_str() {
                        self.add_global(name);
                    }
                }
            }
            "Nonlocal" => {
                for name_val in ast_list(stmt, "names") {
                    if let Some(name) = name_val.as_str() {
                        self.add_def(name, DEF_NONLOCAL);
                    }
                }
            }
            "Expr" => {
                if let Some(val) = ast_field(stmt, "value") {
                    self.visit_expr(&val);
                }
            }
            "Pass" | "Break" | "Continue" => {}
            _ => {}
        }
    }

    fn visit_type_params(
        &mut self,
        type_params: &[Value],
        owner_name: &str,
        lineno: i64,
        visit_body: impl FnOnce(&mut Self),
    ) {
        self.push_block(
            TYPE_TYPE_PARAM,
            owner_name.to_string(),
            lineno,
            true,
            false,
            None,
        );
        for tp in type_params {
            if let Some(name) = ast_str(tp, "name") {
                self.add_def(&name, DEF_LOCAL);
            }
            if let Some(bound) = ast_field(tp, "bound")
                && !bound.is_none()
            {
                let tp_name = ast_str(tp, "name").unwrap_or_default();
                let tp_line = ast_int(tp, "lineno");
                self.push_block(TYPE_TYPE_VAR_BOUND, tp_name, tp_line, true, false, None);
                self.visit_expr(&bound);
                self.pop_block();
            }
        }
        visit_body(self);
        self.pop_block();
    }

    fn visit_arg_defaults(&mut self, args: &Value) {
        for d in ast_list(args, "defaults") {
            self.visit_expr(&d);
        }
        for d in ast_list(args, "kw_defaults") {
            if !d.is_none() {
                self.visit_expr(&d);
            }
        }
    }

    fn visit_arg_annotations(&mut self, args: &Value) {
        for list_field in ["posonlyargs", "args", "kwonlyargs"] {
            for arg in ast_list(args, list_field) {
                if let Some(ann) = ast_field(&arg, "annotation")
                    && !ann.is_none()
                {
                    self.visit_expr(&ann);
                }
            }
        }
        for opt_field in ["vararg", "kwarg"] {
            if let Some(arg) = ast_field(args, opt_field)
                && !arg.is_none()
                && let Some(ann) = ast_field(&arg, "annotation")
                && !ann.is_none()
            {
                self.visit_expr(&ann);
            }
        }
    }

    fn visit_arg_names(&mut self, args: &Value) {
        // CPython 3.12 parameter order: posonlyargs, args, kwonlyargs, vararg, kwarg.
        for list_field in ["posonlyargs", "args", "kwonlyargs"] {
            for arg in ast_list(args, list_field) {
                if let Some(name) = ast_str(&arg, "arg") {
                    let mangled = self.mangle(&name);
                    self.current_mut().add_flag(&mangled, DEF_PARAM);
                    self.current_mut().varnames.push(mangled);
                }
            }
        }
        for opt_field in ["vararg", "kwarg"] {
            if let Some(arg) = ast_field(args, opt_field)
                && !arg.is_none()
                && let Some(name) = ast_str(&arg, "arg")
            {
                let mangled = self.mangle(&name);
                self.current_mut().add_flag(&mangled, DEF_PARAM);
                self.current_mut().varnames.push(mangled);
            }
        }
    }

    fn visit_pattern(&mut self, pat: &Value) {
        let Some(kind) = ast_class_name(pat) else {
            return;
        };
        match kind.as_str() {
            "MatchValue" => {
                if let Some(v) = ast_field(pat, "value") {
                    self.visit_expr(&v);
                }
            }
            "MatchSingleton" => {}
            "MatchSequence" => {
                for p in ast_list(pat, "patterns") {
                    self.visit_pattern(&p);
                }
            }
            "MatchMapping" => {
                for k in ast_list(pat, "keys") {
                    self.visit_expr(&k);
                }
                for p in ast_list(pat, "patterns") {
                    self.visit_pattern(&p);
                }
                if let Some(rest) = ast_str(pat, "rest") {
                    self.add_def(&rest, DEF_LOCAL);
                }
            }
            "MatchClass" => {
                if let Some(cls) = ast_field(pat, "cls") {
                    self.visit_expr(&cls);
                }
                for p in ast_list(pat, "patterns") {
                    self.visit_pattern(&p);
                }
                for p in ast_list(pat, "kwd_patterns") {
                    self.visit_pattern(&p);
                }
            }
            "MatchStar" => {
                if let Some(name) = ast_str(pat, "name") {
                    self.add_def(&name, DEF_LOCAL);
                }
            }
            "MatchAs" => {
                if let Some(p) = ast_field(pat, "pattern")
                    && !p.is_none()
                {
                    self.visit_pattern(&p);
                }
                if let Some(name) = ast_str(pat, "name") {
                    self.add_def(&name, DEF_LOCAL);
                }
            }
            "MatchOr" => {
                for p in ast_list(pat, "patterns") {
                    self.visit_pattern(&p);
                }
            }
            _ => {}
        }
    }

    fn visit_expr(&mut self, expr: &Value) {
        let Some(kind) = ast_class_name(expr) else {
            return;
        };
        match kind.as_str() {
            "Name" => {
                let id = ast_str(expr, "id").unwrap_or_default();
                let ctx_kind = ast_field(expr, "ctx")
                    .and_then(|c| ast_class_name(&c))
                    .unwrap_or_default();
                match ctx_kind.as_str() {
                    "Store" | "Del" => self.add_def(&id, DEF_LOCAL),
                    _ => self.add_def(&id, USE),
                }
            }
            "NamedExpr" => {
                if let Some(val) = ast_field(expr, "value") {
                    self.visit_expr(&val);
                }
                if let Some(target) = ast_field(expr, "target")
                    && let Some(id) = ast_str(&target, "id")
                {
                    self.add_walrus_target(&id);
                }
            }
            "BoolOp" => {
                for v in ast_list(expr, "values") {
                    self.visit_expr(&v);
                }
            }
            "BinOp" => {
                if let Some(l) = ast_field(expr, "left") {
                    self.visit_expr(&l);
                }
                if let Some(r) = ast_field(expr, "right") {
                    self.visit_expr(&r);
                }
            }
            "UnaryOp" => {
                if let Some(o) = ast_field(expr, "operand") {
                    self.visit_expr(&o);
                }
            }
            "Lambda" => {
                let lineno = ast_int(expr, "lineno");
                let args_node = ast_field(expr, "args");
                if let Some(ref args) = args_node {
                    self.visit_arg_defaults(args);
                }
                self.push_block(
                    TYPE_FUNCTION,
                    "lambda".to_string(),
                    lineno,
                    true,
                    false,
                    None,
                );
                if let Some(ref args) = args_node {
                    self.visit_arg_names(args);
                }
                if let Some(body) = ast_field(expr, "body") {
                    self.visit_expr(&body);
                }
                self.pop_block();
            }
            "IfExp" => {
                if let Some(t) = ast_field(expr, "test") {
                    self.visit_expr(&t);
                }
                if let Some(b) = ast_field(expr, "body") {
                    self.visit_expr(&b);
                }
                if let Some(o) = ast_field(expr, "orelse") {
                    self.visit_expr(&o);
                }
            }
            "Dict" => {
                for k in ast_list(expr, "keys") {
                    if !k.is_none() {
                        self.visit_expr(&k);
                    }
                }
                for v in ast_list(expr, "values") {
                    self.visit_expr(&v);
                }
            }
            "Set" | "List" | "Tuple" => {
                for e in ast_list(expr, "elts") {
                    self.visit_expr(&e);
                }
            }
            "ListComp" | "SetComp" | "DictComp" => {
                let generators = ast_list(expr, "generators");
                let has_async = generators.iter().any(|g| ast_bool(g, "is_async"));
                // PEP 709 (Python 3.12): synchronous list/set/dict comprehensions
                // are inlined into the enclosing scope; async comprehensions use
                // a separate comprehension scope.
                if !has_async {
                    self.visit_comprehension_inlined(expr, &kind, &generators);
                } else {
                    let cname = match kind.as_str() {
                        "ListComp" => "listcomp",
                        "SetComp" => "setcomp",
                        _ => "dictcomp",
                    };
                    self.visit_comprehension_block(expr, &kind, cname, &generators);
                }
            }
            "GeneratorExp" => {
                let generators = ast_list(expr, "generators");
                self.visit_comprehension_block(expr, "GeneratorExp", "genexpr", &generators);
            }
            "Await" | "YieldFrom" | "Starred" => {
                if let Some(v) = ast_field(expr, "value") {
                    self.visit_expr(&v);
                }
            }
            "Yield" => {
                if let Some(v) = ast_field(expr, "value")
                    && !v.is_none()
                {
                    self.visit_expr(&v);
                }
            }
            "Compare" => {
                if let Some(l) = ast_field(expr, "left") {
                    self.visit_expr(&l);
                }
                for c in ast_list(expr, "comparators") {
                    self.visit_expr(&c);
                }
            }
            "Call" => {
                if let Some(f) = ast_field(expr, "func") {
                    self.visit_expr(&f);
                }
                for a in ast_list(expr, "args") {
                    self.visit_expr(&a);
                }
                for kw in ast_list(expr, "keywords") {
                    if let Some(v) = ast_field(&kw, "value") {
                        self.visit_expr(&v);
                    }
                }
            }
            "FormattedValue" => {
                if let Some(v) = ast_field(expr, "value") {
                    self.visit_expr(&v);
                }
                if let Some(spec) = ast_field(expr, "format_spec")
                    && !spec.is_none()
                {
                    self.visit_expr(&spec);
                }
            }
            "JoinedStr" => {
                for v in ast_list(expr, "values") {
                    self.visit_expr(&v);
                }
            }
            "Attribute" => {
                if let Some(v) = ast_field(expr, "value") {
                    self.visit_expr(&v);
                }
            }
            "Subscript" => {
                if let Some(v) = ast_field(expr, "value") {
                    self.visit_expr(&v);
                }
                if let Some(s) = ast_field(expr, "slice") {
                    self.visit_expr(&s);
                }
            }
            "Slice" => {
                for f in ["lower", "upper", "step"] {
                    if let Some(v) = ast_field(expr, f)
                        && !v.is_none()
                    {
                        self.visit_expr(&v);
                    }
                }
            }
            "Constant" => {}
            _ => {}
        }
    }

    fn visit_comprehension_inlined(&mut self, expr: &Value, kind: &str, generators: &[Value]) {
        // Outermost iterator is evaluated directly in the enclosing block;
        // comprehension targets/body symbols are merged at the end of the
        // enclosing block to match CPython 3.12's PEP 709 symbol ordering.
        if let Some(first) = generators.first()
            && let Some(iter) = ast_field(first, "iter")
        {
            self.visit_expr(&iter);
        }
        let lineno = ast_int(expr, "lineno");
        self.push_block(TYPE_FUNCTION, String::new(), lineno, true, true, None);
        for (idx, comp_gen) in generators.iter().enumerate() {
            if idx > 0
                && let Some(iter) = ast_field(comp_gen, "iter")
            {
                self.visit_expr(&iter);
            }
            if let Some(target) = ast_field(comp_gen, "target") {
                self.visit_expr(&target);
            }
            for cond in ast_list(comp_gen, "ifs") {
                self.visit_expr(&cond);
            }
        }
        if kind == "DictComp" {
            if let Some(k) = ast_field(expr, "key") {
                self.visit_expr(&k);
            }
            if let Some(v) = ast_field(expr, "value") {
                self.visit_expr(&v);
            }
        } else if let Some(elt) = ast_field(expr, "elt") {
            self.visit_expr(&elt);
        }
        let mut temp = self.stack.pop().expect("inlined comp block");
        temp.flush_inlined_comp_symbols();
        let parent = self.current_mut();
        for (name, flag) in temp.symbols {
            let entry = parent.inlined_comp_symbols.entry(name).or_insert(0);
            *entry |= flag;
        }
        parent.children.extend(temp.children);
    }

    fn visit_comprehension_block(
        &mut self,
        expr: &Value,
        kind: &str,
        block_name: &str,
        generators: &[Value],
    ) {
        let lineno = ast_int(expr, "lineno");
        // The outermost iterator is evaluated in the enclosing scope.
        if let Some(first) = generators.first()
            && let Some(iter) = ast_field(first, "iter")
        {
            self.visit_expr(&iter);
        }
        self.push_block(
            TYPE_FUNCTION,
            block_name.to_string(),
            lineno,
            true,
            true,
            None,
        );
        self.current_mut().add_flag(".0", DEF_PARAM);
        self.current_mut().varnames.push(".0".to_string());
        for (idx, comp_gen) in generators.iter().enumerate() {
            if idx > 0
                && let Some(iter) = ast_field(comp_gen, "iter")
            {
                self.visit_expr(&iter);
            }
            if let Some(target) = ast_field(comp_gen, "target") {
                self.visit_expr(&target);
            }
            for cond in ast_list(comp_gen, "ifs") {
                self.visit_expr(&cond);
            }
        }
        if kind == "DictComp" {
            if let Some(k) = ast_field(expr, "key") {
                self.visit_expr(&k);
            }
            if let Some(v) = ast_field(expr, "value") {
                self.visit_expr(&v);
            }
        } else if let Some(elt) = ast_field(expr, "elt") {
            self.visit_expr(&elt);
        }
        self.pop_block();
    }
}

// ── Pass 2: Lexical Scope Resolution (CPython `analyze_block`) ───────────────

fn analyze_block(
    block: &mut WorkBlock,
    bound: &HashSet<String>,
    free: &mut HashSet<String>,
    global: &HashSet<String>,
) {
    let mut local: HashSet<String> = HashSet::new();
    let mut newglobal: HashSet<String> = HashSet::new();
    let mut newbound: HashSet<String> = HashSet::new();
    let mut scopes: HashMap<String, i64> = HashMap::new();

    if block.block_type == TYPE_CLASS {
        newglobal.extend(global.iter().cloned());
        newbound.extend(bound.iter().cloned());
    }

    // Classify every symbol directly mentioned in this block.
    for (name, &flags) in &block.symbols {
        let scope = if (flags & DEF_GLOBAL) != 0 {
            newglobal.insert(name.clone());
            GLOBAL_EXPLICIT
        } else if (flags & DEF_NONLOCAL) != 0 {
            free.insert(name.clone());
            FREE
        } else if (flags & DEF_BOUND) != 0 {
            local.insert(name.clone());
            LOCAL
        } else if bound.contains(name) {
            free.insert(name.clone());
            FREE
        } else {
            GLOBAL_IMPLICIT
        };
        scopes.insert(name.clone(), scope);
    }

    // Prepare inherited sets for child blocks.
    if block.block_type != TYPE_CLASS {
        if matches!(
            block.block_type,
            TYPE_FUNCTION
                | TYPE_ANNOTATION
                | TYPE_TYPE_ALIAS
                | TYPE_TYPE_PARAM
                | TYPE_TYPE_VAR_BOUND
        ) {
            newbound.extend(local.iter().cloned());
        }
        newbound.extend(bound.iter().cloned());
        newglobal.extend(global.iter().cloned());
        // Locals in a function shadow enclosing globals for nested scopes.
        if block.block_type == TYPE_FUNCTION {
            for l in &local {
                newglobal.remove(l);
            }
        }
    } else {
        // Class bodies provide `__class__` for zero-arg `super()`.
        newbound.insert("__class__".to_string());
    }

    // Recursively analyze children and collect their free variables.
    let mut allfree: HashSet<String> = HashSet::new();
    for child in &mut block.children {
        let mut child_free: HashSet<String> = HashSet::new();
        analyze_block(child, &newbound, &mut child_free, &newglobal);
        allfree.extend(child_free);
    }

    // Propagate child free variables back up: promote matching locals to CELL,
    // or mark as FREE to continue propagating to outer scopes.
    for name in &allfree {
        if block.block_type == TYPE_CLASS {
            if name == "__class__" {
                continue;
            }
            if bound.contains(name) {
                free.insert(name.clone());
                if let Some(flags) = block.symbols.get_mut(name) {
                    *flags |= DEF_FREE_CLASS;
                }
            }
            continue;
        }
        if matches!(
            block.block_type,
            TYPE_FUNCTION
                | TYPE_ANNOTATION
                | TYPE_TYPE_ALIAS
                | TYPE_TYPE_PARAM
                | TYPE_TYPE_VAR_BOUND
        ) {
            if local.contains(name) {
                scopes.insert(name.clone(), CELL);
            } else if bound.contains(name) && !block.explicit_globals.contains(name) {
                free.insert(name.clone());
                scopes.insert(name.clone(), FREE);
                block
                    .symbols
                    .entry(name.clone())
                    .and_modify(|f| *f |= DEF_FREE)
                    .or_insert(DEF_FREE);
            }
        }
    }

    // Commit final scope nibble into each symbol's flags integer.
    for (name, flags) in &mut block.symbols {
        let scope = scopes.get(name).copied().unwrap_or(GLOBAL_IMPLICIT);
        *flags = (*flags & !((SCOPE_MASK) << SCOPE_OFF)) | (scope << SCOPE_OFF);
    }
}

/// Parse and analyze Python source into a `RawSymTable` tree.
pub(crate) fn analyze_source(
    interp: &mut Interpreter,
    code: &Value,
    filename: &str,
    compile_type: &str,
) -> Result<Rc<RawSymTable>> {
    if !matches!(compile_type, "exec" | "eval" | "single") {
        return Err(PyError::named(
            "ValueError",
            "symtable() arg 3 must be 'exec', 'eval' or 'single'".to_string(),
        ));
    }
    let source_str = match code.kind() {
        ValueKind::Str(s) => s.to_string(),
        ValueKind::Bytes(b) => std::str::from_utf8(b)
            .map_err(|e| PyError::named("SyntaxError", format!("invalid utf-8: {e}")))?
            .to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "symtable() arg 1 must be a string, not '{}'",
                    value_type_name_str(code)
                ),
            ));
        }
    };

    // Parse via PyRust's full Python 3.12 `ast.parse` for exact syntax checks
    // and line numbers.
    let ast_mod = interp.load_module("ast")?;
    let parse_fn = interp.get_attr(&ast_mod, "parse")?;
    let tree = interp.call_function_expanded(
        parse_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(source_str),
            },
            ExpandedCallArg {
                name: Some("filename".to_string()),
                value: Value::string(filename),
            },
            ExpandedCallArg {
                name: Some("mode".to_string()),
                value: Value::string(compile_type),
            },
        ],
    )?;

    let mut analyzer = AstAnalyzer::new();
    analyzer.visit_module(&tree);
    let mut root = analyzer.stack.pop().expect("root block");
    root.flush_inlined_comp_symbols();
    let bound = HashSet::new();
    let mut free = HashSet::new();
    let global = HashSet::new();
    analyze_block(&mut root, &bound, &mut free, &global);
    Ok(root.into_raw())
}

fn wrap_raw_entry(interp: &mut Interpreter, raw: Rc<RawSymTable>) -> Result<Value> {
    let mod_val = interp.load_module("_symtable")?;
    let cls = interp.get_attr(&mod_val, "symtable_entry")?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::runtime("corrupt _symtable.symtable_entry".into()));
    };
    let inst = Value::py_instance(Rc::new(std::cell::RefCell::new(crate::value::PyInstance {
        class: Rc::clone(cls_rc),
        attrs: crate::value::InstanceAttrs::new(),
    })));
    crate::builtin_modules::macros::native_state::set_native_state(&inst, raw)?;
    Ok(inst)
}

pub(crate) fn extract_raw_entry(val: &Value) -> Result<Rc<RawSymTable>> {
    crate::builtin_modules::macros::native_state::with_native_state::<Rc<RawSymTable>, _>(
        val,
        "symtable_entry",
        |r| Ok(Rc::clone(r)),
    )
}

pyrust_module! {
    constants {
        "USE" => Value::int(USE),
        "DEF_GLOBAL" => Value::int(DEF_GLOBAL),
        "DEF_NONLOCAL" => Value::int(DEF_NONLOCAL),
        "DEF_LOCAL" => Value::int(DEF_LOCAL),
        "DEF_PARAM" => Value::int(DEF_PARAM),
        "DEF_FREE" => Value::int(DEF_FREE),
        "DEF_FREE_CLASS" => Value::int(DEF_FREE_CLASS),
        "DEF_IMPORT" => Value::int(DEF_IMPORT),
        "DEF_BOUND" => Value::int(DEF_BOUND),
        "DEF_ANNOT" => Value::int(DEF_ANNOT),
        "TYPE_FUNCTION" => Value::int(TYPE_FUNCTION),
        "TYPE_CLASS" => Value::int(TYPE_CLASS),
        "TYPE_MODULE" => Value::int(TYPE_MODULE),
        "TYPE_ANNOTATION" => Value::int(TYPE_ANNOTATION),
        "TYPE_TYPE_VAR_BOUND" => Value::int(TYPE_TYPE_VAR_BOUND),
        "TYPE_TYPE_ALIAS" => Value::int(TYPE_TYPE_ALIAS),
        "TYPE_TYPE_PARAM" => Value::int(TYPE_TYPE_PARAM),
        "LOCAL" => Value::int(LOCAL),
        "GLOBAL_EXPLICIT" => Value::int(GLOBAL_EXPLICIT),
        "GLOBAL_IMPLICIT" => Value::int(GLOBAL_IMPLICIT),
        "FREE" => Value::int(FREE),
        "CELL" => Value::int(CELL),
        "SCOPE_OFF" => Value::int(SCOPE_OFF),
        "SCOPE_MASK" => Value::int(SCOPE_MASK),
    }

    #[native_state(Rc<RawSymTable>)]
    class symtable_entry {
        #[getter]
        fn id(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::int(raw.id))
        }

        #[getter("type")]
        fn block_type(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::int(raw.block_type))
        }

        #[getter]
        fn name(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::string(&raw.name))
        }

        #[getter]
        fn lineno(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::int(raw.lineno))
        }

        #[getter]
        fn optimized(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::bool_(raw.optimized))
        }

        #[getter]
        fn nested(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::bool_(raw.nested))
        }

        #[getter]
        fn has_children(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::bool_(!raw.children.is_empty()))
        }

        #[getter]
        fn symbols(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            let mut d = PyDict::default();
            for (k, &v) in &raw.symbols {
                d.insert(PyKey::str_from(k), Value::int(v));
            }
            Ok(Value::dict(d))
        }

        #[getter]
        fn varnames(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::tuple(
                raw.varnames.iter().map(Value::string).collect(),
            ))
        }

        #[getter]
        fn children(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let children: Vec<Rc<RawSymTable>> = with_state(self_val.0, |raw| Ok(raw.children.clone()))?;
            let mut out = Vec::with_capacity(children.len());
            for child in children {
                out.push(wrap_raw_entry(_interp, child)?);
            }
            Ok(Value::list(out))
        }

        fn __repr__(#[state] raw: &Rc<RawSymTable>) -> Result<Value> {
            Ok(Value::string(format!(
                "<symtable entry {}({}), line {}>",
                raw.name, raw.id, raw.lineno
            )))
        }
    }

    fn symtable(
        #[positional_only] code: PyValue,
        #[positional_only] filename: PyPath,
        #[positional_only] compile_type: PyStr<'_>,
    ) -> Result<Value> {
        let fname = filename.as_path().to_string_lossy().into_owned();
        let raw = analyze_source(_interp, &code.0, &fname, &compile_type.0)?;
        wrap_raw_entry(_interp, raw)
    }
}
