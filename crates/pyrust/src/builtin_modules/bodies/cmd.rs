// `cmd` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/cmd.py` (issues #3228, #3477).
//
// The module mirrors the CPython source: the `Cmd` class with its class-level
// defaults (`prompt`, `identchars`, `ruler`, `lastcmd`, `intro`, `doc_leader`,
// `doc_header`, `misc_header`, `undoc_header`, `nohelp`, `use_rawinput`), the
// `PROMPT` / `IDENTCHARS` constants and `__all__`.
//
// Like the Python source, every step goes back through `self`: the hooks
// (`preloop`, `postloop`, `precmd`, `postcmd`), `parseline`, `onecmd`,
// `emptyline`, `default`, `completedefault`, `completenames`, `complete`,
// `get_names`, `complete_help`, `do_help`, `print_topics`, `columnize`, and
// the dynamically looked-up `do_<cmd>` / `help_<topic>` / `complete_<cmd>`
// methods and the instance / class attributes are all resolved with ordinary
// attribute lookups, so subclasses overriding any of them (or any class
// attribute) behave exactly as in CPython.  `readline` is imported lazily on
// every use, the way the source's function-local `import readline` does.
//
// Reference: <https://docs.python.org/3.12/library/cmd.html>

use std::collections::BTreeSet;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

#[path = "cmd/docs.rs"]
mod docs;

/// `PROMPT`.
const PROMPT: &str = "(Cmd) ";
/// `IDENTCHARS = string.ascii_letters + string.digits + '_'`.
const IDENTCHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_";

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

// The `def`s of `Lib/cmd.py`: CPython reports arity errors for these in the
// interpreter's `initialize_locals` wording, not the macro's.
const SIG_INIT: Sig = sig(
    "Cmd.__init__",
    &["self", "completekey", "stdin", "stdout"],
    3,
);
const SIG_CMDLOOP: Sig = sig("Cmd.cmdloop", &["self", "intro"], 1);
const SIG_PRECMD: Sig = sig("Cmd.precmd", &["self", "line"], 0);
const SIG_POSTCMD: Sig = sig("Cmd.postcmd", &["self", "stop", "line"], 0);
const SIG_PRELOOP: Sig = sig("Cmd.preloop", &["self"], 0);
const SIG_POSTLOOP: Sig = sig("Cmd.postloop", &["self"], 0);
const SIG_PARSELINE: Sig = sig("Cmd.parseline", &["self", "line"], 0);
const SIG_ONECMD: Sig = sig("Cmd.onecmd", &["self", "line"], 0);
const SIG_EMPTYLINE: Sig = sig("Cmd.emptyline", &["self"], 0);
const SIG_DEFAULT: Sig = sig("Cmd.default", &["self", "line"], 0);
// `def completedefault(self, *ignored)`.
const SIG_COMPLETEDEFAULT: Sig = sig("Cmd.completedefault", &["self"], 0);
// `def completenames(self, text, *ignored)`.
const SIG_COMPLETENAMES: Sig = sig("Cmd.completenames", &["self", "text"], 0);
const SIG_COMPLETE: Sig = sig("Cmd.complete", &["self", "text", "state"], 0);
const SIG_GET_NAMES: Sig = sig("Cmd.get_names", &["self"], 0);
// `def complete_help(self, *args)`.
const SIG_COMPLETE_HELP: Sig = sig("Cmd.complete_help", &["self"], 0);
const SIG_DO_HELP: Sig = sig("Cmd.do_help", &["self", "arg"], 0);
const SIG_PRINT_TOPICS: Sig = sig(
    "Cmd.print_topics",
    &["self", "header", "cmds", "cmdlen", "maxcol"],
    0,
);
const SIG_COLUMNIZE: Sig = sig("Cmd.columnize", &["self", "list", "displaywidth"], 1);

fn all_list() -> Value {
    Value::list(vec![Value::string("Cmd")])
}

// ---------------------------------------------------------------------------
// Interpreter plumbing
// ---------------------------------------------------------------------------

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, pos)
}

/// A builtin function (`len`, `input`, `dir`, ...), the way the source reaches
/// it through the builtins namespace.
fn builtin(interp: &mut Interpreter, name: &'static str, pos: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), pos)
}

/// A module-level dependency (`sys`): the module attribute when present
/// (CPython resolves `import sys` as a module global), otherwise the imported
/// module itself.
fn dep(interp: &mut Interpreter, name: &str) -> Result<Value> {
    if let Ok(module) = interp.load_module("cmd")
        && let Ok(value) = interp.get_attr(&module, name)
    {
        return Ok(value);
    }
    interp.load_module(name)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

fn binary(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<Value> {
    interp.eval_binary(a.clone(), op, b.clone())
}

fn compare(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<bool> {
    let result = binary(interp, a, op, b)?;
    truthy(interp, &result)
}

/// `a == b`.
fn values_equal(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    compare(interp, a, BinaryOp::Eq, b)
}

/// `fmt % value` with CPython's `%` formatting semantics.
fn percent_format(interp: &mut Interpreter, fmt: &str, value: Value) -> Result<Value> {
    interp.eval_binary(Value::string(fmt), BinaryOp::Mod, value)
}

/// `str(value)`.
fn to_str(interp: &mut Interpreter, value: Value) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    let text = percent_format(interp, "%s", Value::tuple(vec![value]))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn is_import_error(err: &PyError) -> bool {
    err.class_name_is("ImportError") || err.class_name_is("ModuleNotFoundError")
}

fn is_attribute_error(err: &PyError) -> bool {
    err.class_name_is("AttributeError")
}

/// Run `f`.  An error accepted by `swallow` is handed back as `Ok(Err(err))`
/// after the traceback frames recorded while it unwound are dropped, so the
/// swallowed exception (CPython's `except` clause) does not leak frames into a
/// later, unrelated exception.  Any other error propagates unchanged.
fn try_swallow<T>(
    f: impl FnOnce() -> Result<T>,
    swallow: impl FnOnce(&PyError) -> bool,
) -> Result<std::result::Result<T, PyError>> {
    let saved = pyrust_core::clone_captured_error_frames();
    match f() {
        Ok(value) => Ok(Ok(value)),
        Err(err) if swallow(&err) => {
            pyrust_core::reset_captured_error_frames();
            for frame in saved.into_iter().rev() {
                pyrust_core::record_traceback_frame(frame);
            }
            Ok(Err(err))
        }
        Err(err) => Err(err),
    }
}

/// `self.stdout.write(text)`.
fn write_out(interp: &mut Interpreter, this: &Value, text: Value) -> Result<()> {
    let stdout = interp.get_attr(this, "stdout")?;
    call_method(interp, &stdout, "write", &[text])?;
    Ok(())
}

/// `a, b, c = value` for exactly three items.
fn unpack3(interp: &mut Interpreter, value: &Value) -> Result<[Value; 3]> {
    let items = interp.collect_iterable(value)?;
    match items.len() {
        3 => {
            let mut it = items.into_iter();
            Ok([
                it.next().unwrap_or_else(Value::none),
                it.next().unwrap_or_else(Value::none),
                it.next().unwrap_or_else(Value::none),
            ])
        }
        n if n < 3 => Err(PyError::named(
            "ValueError",
            format!("not enough values to unpack (expected 3, got {n})"),
        )),
        _ => Err(PyError::named(
            "ValueError",
            "too many values to unpack (expected 3)",
        )),
    }
}

/// The characters of a `str` value (`str` subclass instances included).
fn chars_of(interp: &mut Interpreter, value: &Value) -> Result<Vec<char>> {
    Ok(to_str(interp, value.clone())?.chars().collect())
}

/// `import readline`, honouring `sys.modules`.
fn import_readline(interp: &mut Interpreter) -> Result<Value> {
    interp.load_module("readline")
}

/// Drop the surplus positional arguments of a `*ignored` / `*args` method,
/// keeping the first `keep` positionals and every keyword argument.
fn drop_varargs(args: &[ExpandedCallArg], keep: usize) -> Vec<ExpandedCallArg> {
    let mut positional = 0usize;
    args.iter()
        .filter(|arg| {
            if arg.name.is_some() {
                return true;
            }
            positional += 1;
            positional <= keep
        })
        .cloned()
        .collect()
}

/// The positional (non-keyword) arguments, in order.
fn positionals(args: &[ExpandedCallArg]) -> Vec<Value> {
    args.iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| arg.value.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Cmd
// ---------------------------------------------------------------------------

/// `Cmd.__init__(self, completekey='tab', stdin=None, stdout=None)`.
fn cmd_init(
    interp: &mut Interpreter,
    this: &Value,
    completekey: Value,
    stdin: Value,
    stdout: Value,
) -> Result<()> {
    let stdin = if stdin.is_none() {
        let sys = dep(interp, "sys")?;
        interp.get_attr(&sys, "stdin")?
    } else {
        stdin
    };
    interp.assign_attr(this.clone(), "stdin", stdin)?;
    let stdout = if stdout.is_none() {
        let sys = dep(interp, "sys")?;
        interp.get_attr(&sys, "stdout")?
    } else {
        stdout
    };
    interp.assign_attr(this.clone(), "stdout", stdout)?;
    interp.assign_attr(this.clone(), "cmdqueue", Value::list(Vec::new()))?;
    interp.assign_attr(this.clone(), "completekey", completekey)?;
    Ok(())
}

/// `self.use_rawinput and self.completekey`.
fn completion_enabled(interp: &mut Interpreter, this: &Value) -> Result<bool> {
    let raw = interp.get_attr(this, "use_rawinput")?;
    if !truthy(interp, &raw)? {
        return Ok(false);
    }
    let key = interp.get_attr(this, "completekey")?;
    truthy(interp, &key)
}

/// The `try: import readline ... except ImportError: pass` block that installs
/// `self.complete` as the readline completer.
fn install_completer(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let _ = try_swallow(
        || -> Result<()> {
            let readline = import_readline(interp)?;
            let old = call_method(interp, &readline, "get_completer", &[])?;
            interp.assign_attr(this.clone(), "old_completer", old)?;
            let complete = interp.get_attr(this, "complete")?;
            call_method(interp, &readline, "set_completer", &[complete])?;
            let key = interp.get_attr(this, "completekey")?;
            let binding = binary(interp, &key, BinaryOp::Add, &Value::string(": complete"))?;
            call_method(interp, &readline, "parse_and_bind", &[binding])?;
            Ok(())
        },
        is_import_error,
    )?;
    Ok(())
}

/// The `finally:` block of `cmdloop`: restore the previous completer.
fn restore_completer(interp: &mut Interpreter, this: &Value) -> Result<()> {
    if !completion_enabled(interp, this)? {
        return Ok(());
    }
    let _ = try_swallow(
        || -> Result<()> {
            let readline = import_readline(interp)?;
            let old = interp.get_attr(this, "old_completer")?;
            call_method(interp, &readline, "set_completer", &[old])?;
            Ok(())
        },
        is_import_error,
    )?;
    Ok(())
}

/// The body of `cmdloop`'s `try:` block.
fn cmdloop_body(interp: &mut Interpreter, this: &Value, intro: &Value) -> Result<()> {
    if !intro.is_none() {
        interp.assign_attr(this.clone(), "intro", intro.clone())?;
    }
    let own_intro = interp.get_attr(this, "intro")?;
    if truthy(interp, &own_intro)? {
        let text = to_str(interp, own_intro)?;
        write_out(interp, this, Value::string(format!("{text}\n")))?;
    }
    let mut stop = Value::none();
    while !truthy(interp, &stop)? {
        let queue = interp.get_attr(this, "cmdqueue")?;
        let line = if truthy(interp, &queue)? {
            let queue = interp.get_attr(this, "cmdqueue")?;
            call_method(interp, &queue, "pop", &[Value::int(0)])?
        } else {
            let raw = interp.get_attr(this, "use_rawinput")?;
            if truthy(interp, &raw)? {
                let prompt = interp.get_attr(this, "prompt")?;
                match try_swallow(
                    || builtin(interp, "input", &[prompt]),
                    |err| err.class_name_is("EOFError"),
                )? {
                    Ok(line) => line,
                    Err(_) => Value::string("EOF"),
                }
            } else {
                let prompt = interp.get_attr(this, "prompt")?;
                write_out(interp, this, prompt)?;
                let stdout = interp.get_attr(this, "stdout")?;
                call_method(interp, &stdout, "flush", &[])?;
                let stdin = interp.get_attr(this, "stdin")?;
                let line = call_method(interp, &stdin, "readline", &[])?;
                let length = builtin(interp, "len", std::slice::from_ref(&line))?;
                if !truthy(interp, &length)? {
                    Value::string("EOF")
                } else {
                    call_method(interp, &line, "rstrip", &[Value::string("\r\n")])?
                }
            }
        };
        let line = call_method(interp, this, "precmd", &[line])?;
        stop = call_method(interp, this, "onecmd", std::slice::from_ref(&line))?;
        stop = call_method(interp, this, "postcmd", &[stop, line])?;
    }
    call_method(interp, this, "postloop", &[])?;
    Ok(())
}

/// `Cmd.cmdloop(self, intro=None)`.
fn cmdloop(interp: &mut Interpreter, this: &Value, intro: &Value) -> Result<()> {
    call_method(interp, this, "preloop", &[])?;
    if completion_enabled(interp, this)? {
        install_completer(interp, this)?;
    }
    let outcome = cmdloop_body(interp, this, intro);
    // `finally:` — an error raised here replaces the one in flight.
    restore_completer(interp, this)?;
    outcome
}

/// `Cmd.parseline(self, line)`.
fn parseline(interp: &mut Interpreter, this: &Value, line: &Value) -> Result<Value> {
    let stripped = call_method(interp, line, "strip", &[])?;
    if !truthy(interp, &stripped)? {
        return Ok(Value::tuple(vec![Value::none(), Value::none(), stripped]));
    }
    let mut chars = chars_of(interp, &stripped)?;
    match chars[0] {
        '?' => {
            let mut rewritten: Vec<char> = "help ".chars().collect();
            rewritten.extend_from_slice(&chars[1..]);
            chars = rewritten;
        }
        '!' => {
            let has_shell =
                try_swallow(|| interp.get_attr(this, "do_shell"), is_attribute_error)?.is_ok();
            if has_shell {
                let mut rewritten: Vec<char> = "shell ".chars().collect();
                rewritten.extend_from_slice(&chars[1..]);
                chars = rewritten;
            } else {
                return Ok(Value::tuple(vec![Value::none(), Value::none(), stripped]));
            }
        }
        _ => {}
    }
    let line_text: String = chars.iter().collect();
    let identchars = interp.get_attr(this, "identchars")?;
    let mut i = 0usize;
    while i < chars.len() {
        let ch = Value::string(chars[i].to_string());
        let found = match identchars.as_str() {
            Some(set) => set.contains(chars[i]),
            None => compare(interp, &ch, BinaryOp::In, &identchars)?,
        };
        if !found {
            break;
        }
        i += 1;
    }
    let cmd: String = chars[..i].iter().collect();
    let rest = Value::string(chars[i..].iter().collect::<String>());
    let arg = call_method(interp, &rest, "strip", &[])?;
    Ok(Value::tuple(vec![
        Value::string(cmd),
        arg,
        Value::string(line_text),
    ]))
}

/// `Cmd.onecmd(self, line)`.
fn onecmd(interp: &mut Interpreter, this: &Value, line: &Value) -> Result<Value> {
    let parsed = call_method(interp, this, "parseline", std::slice::from_ref(line))?;
    let [cmd, arg, line] = unpack3(interp, &parsed)?;
    if !truthy(interp, &line)? {
        return call_method(interp, this, "emptyline", &[]);
    }
    if cmd.is_none() {
        return call_method(interp, this, "default", &[line]);
    }
    interp.assign_attr(this.clone(), "lastcmd", line.clone())?;
    if values_equal(interp, &line, &Value::string("EOF"))? {
        interp.assign_attr(this.clone(), "lastcmd", Value::string(""))?;
    }
    if values_equal(interp, &cmd, &Value::string(""))? {
        return call_method(interp, this, "default", &[line]);
    }
    let attr = binary(interp, &Value::string("do_"), BinaryOp::Add, &cmd)?;
    let attr = to_str(interp, attr)?;
    match try_swallow(|| interp.get_attr(this, &attr), is_attribute_error)? {
        Ok(func) => call_pos(interp, func, &[arg]),
        Err(_) => call_method(interp, this, "default", &[line]),
    }
}

/// `Cmd.emptyline(self)`.
fn emptyline(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let last = interp.get_attr(this, "lastcmd")?;
    if truthy(interp, &last)? {
        let last = interp.get_attr(this, "lastcmd")?;
        return call_method(interp, this, "onecmd", &[last]);
    }
    Ok(Value::none())
}

/// `Cmd.default(self, line)`.
fn default_cmd(interp: &mut Interpreter, this: &Value, line: Value) -> Result<Value> {
    let text = percent_format(interp, "*** Unknown syntax: %s\n", line)?;
    write_out(interp, this, text)?;
    Ok(Value::none())
}

/// `[a[3:] for a in self.get_names() if a.startswith(dotext)]`.
fn completenames(interp: &mut Interpreter, this: &Value, text: &Value) -> Result<Value> {
    let dotext = binary(interp, &Value::string("do_"), BinaryOp::Add, text)?;
    let names = call_method(interp, this, "get_names", &[])?;
    let names = interp.collect_iterable(&names)?;
    let mut out = Vec::new();
    for name in names {
        let starts = call_method(interp, &name, "startswith", std::slice::from_ref(&dotext))?;
        if truthy(interp, &starts)? {
            let chars = chars_of(interp, &name)?;
            let tail: String = chars.iter().skip(3).collect();
            out.push(Value::string(tail));
        }
    }
    Ok(Value::list(out))
}

/// `Cmd.complete(self, text, state)`.
fn complete(interp: &mut Interpreter, this: &Value, text: Value, state: Value) -> Result<Value> {
    if values_equal(interp, &state, &Value::int(0))? {
        let readline = import_readline(interp)?;
        let origline = call_method(interp, &readline, "get_line_buffer", &[])?;
        let line = call_method(interp, &origline, "lstrip", &[])?;
        let origlen = builtin(interp, "len", &[origline])?;
        let linelen = builtin(interp, "len", std::slice::from_ref(&line))?;
        let stripped = binary(interp, &origlen, BinaryOp::Sub, &linelen)?;
        let begidx = call_method(interp, &readline, "get_begidx", &[])?;
        let begidx = binary(interp, &begidx, BinaryOp::Sub, &stripped)?;
        let endidx = call_method(interp, &readline, "get_endidx", &[])?;
        let endidx = binary(interp, &endidx, BinaryOp::Sub, &stripped)?;
        let compfunc = if compare(interp, &begidx, BinaryOp::Gt, &Value::int(0))? {
            let parsed = call_method(interp, this, "parseline", std::slice::from_ref(&line))?;
            let [cmd, _args, _foo] = unpack3(interp, &parsed)?;
            if values_equal(interp, &cmd, &Value::string(""))? {
                interp.get_attr(this, "completedefault")?
            } else {
                let attr = binary(interp, &Value::string("complete_"), BinaryOp::Add, &cmd)?;
                let attr = to_str(interp, attr)?;
                match try_swallow(|| interp.get_attr(this, &attr), is_attribute_error)? {
                    Ok(func) => func,
                    Err(_) => interp.get_attr(this, "completedefault")?,
                }
            }
        } else {
            interp.get_attr(this, "completenames")?
        };
        let matches = call_pos(interp, compfunc, &[text, line, begidx, endidx])?;
        interp.assign_attr(this.clone(), "completion_matches", matches)?;
    }
    let matches = interp.get_attr(this, "completion_matches")?;
    match try_swallow(
        || call_method(interp, &matches, "__getitem__", &[state]),
        |err| err.class_name_is("IndexError"),
    )? {
        Ok(value) => Ok(value),
        Err(_) => Ok(Value::none()),
    }
}

/// `Cmd.get_names(self)`: `dir(self.__class__)`.
fn get_names(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let class = interp.get_attr(this, "__class__")?;
    builtin(interp, "dir", &[class])
}

/// `Cmd.complete_help(self, *args)`.
fn complete_help(interp: &mut Interpreter, this: &Value, args: &[Value]) -> Result<Value> {
    let names = call_method(interp, this, "completenames", args)?;
    let commands = builtin(interp, "set", &[names])?;
    let names = call_method(interp, this, "get_names", &[])?;
    let names = interp.collect_iterable(&names)?;
    let mut topics = Vec::new();
    for name in names {
        let first = args
            .first()
            .cloned()
            .ok_or_else(|| PyError::named("IndexError", "tuple index out of range"))?;
        let prefix = binary(interp, &Value::string("help_"), BinaryOp::Add, &first)?;
        let starts = call_method(interp, &name, "startswith", &[prefix])?;
        if truthy(interp, &starts)? {
            let chars = chars_of(interp, &name)?;
            let tail: String = chars.iter().skip(5).collect();
            topics.push(Value::string(tail));
        }
    }
    let topics = builtin(interp, "set", &[Value::list(topics)])?;
    let union = binary(interp, &commands, BinaryOp::BitOr, &topics)?;
    builtin(interp, "list", &[union])
}

/// `Cmd.do_help(self, arg)`.
fn do_help(interp: &mut Interpreter, this: &Value, arg: &Value) -> Result<Value> {
    if truthy(interp, arg)? {
        // XXX check arg syntax
        let attr = binary(interp, &Value::string("help_"), BinaryOp::Add, arg)?;
        let attr = to_str(interp, attr)?;
        match try_swallow(|| interp.get_attr(this, &attr), is_attribute_error)? {
            Ok(func) => {
                call_pos(interp, func, &[])?;
            }
            Err(_) => {
                let documented = try_swallow(
                    || -> Result<bool> {
                        let attr = binary(interp, &Value::string("do_"), BinaryOp::Add, arg)?;
                        let attr = to_str(interp, attr)?;
                        let func = interp.get_attr(this, &attr)?;
                        let doc = interp.get_attr(&func, "__doc__")?;
                        if truthy(interp, &doc)? {
                            let text = to_str(interp, doc)?;
                            write_out(interp, this, Value::string(format!("{text}\n")))?;
                            return Ok(true);
                        }
                        Ok(false)
                    },
                    is_attribute_error,
                )?;
                if matches!(documented, Ok(true)) {
                    return Ok(Value::none());
                }
                let nohelp = interp.get_attr(this, "nohelp")?;
                let message = binary(
                    interp,
                    &nohelp,
                    BinaryOp::Mod,
                    &Value::tuple(vec![arg.clone()]),
                )?;
                let message = to_str(interp, message)?;
                write_out(interp, this, Value::string(format!("{message}\n")))?;
            }
        }
        return Ok(Value::none());
    }
    let names = call_method(interp, this, "get_names", &[])?;
    let mut cmds_doc: Vec<Value> = Vec::new();
    let mut cmds_undoc: Vec<Value> = Vec::new();
    let mut topics: BTreeSet<String> = BTreeSet::new();
    for name in interp.collect_iterable(&names)? {
        let text = name_text(interp, &name)?;
        if let Some(topic) = text.strip_prefix("help_") {
            topics.insert(topic.to_string());
        }
    }
    call_method(interp, &names, "sort", &[])?;
    // There can be duplicates if routines overridden
    let mut prevname = String::new();
    for name in interp.collect_iterable(&names)? {
        let text = name_text(interp, &name)?;
        let Some(cmd) = text.strip_prefix("do_") else {
            continue;
        };
        if text == prevname {
            continue;
        }
        prevname = text.clone();
        if topics.remove(cmd) {
            cmds_doc.push(Value::string(cmd));
        } else {
            let func = interp.get_attr(this, &text)?;
            let doc = interp.get_attr(&func, "__doc__")?;
            if truthy(interp, &doc)? {
                cmds_doc.push(Value::string(cmd));
            } else {
                cmds_undoc.push(Value::string(cmd));
            }
        }
    }
    let leader = interp.get_attr(this, "doc_leader")?;
    let leader = to_str(interp, leader)?;
    write_out(interp, this, Value::string(format!("{leader}\n")))?;
    let doc_header = interp.get_attr(this, "doc_header")?;
    call_method(
        interp,
        this,
        "print_topics",
        &[
            doc_header,
            Value::list(cmds_doc),
            Value::int(15),
            Value::int(80),
        ],
    )?;
    let misc_header = interp.get_attr(this, "misc_header")?;
    let misc: Vec<Value> = topics.into_iter().map(Value::string).collect();
    call_method(
        interp,
        this,
        "print_topics",
        &[
            misc_header,
            Value::list(misc),
            Value::int(15),
            Value::int(80),
        ],
    )?;
    let undoc_header = interp.get_attr(this, "undoc_header")?;
    call_method(
        interp,
        this,
        "print_topics",
        &[
            undoc_header,
            Value::list(cmds_undoc),
            Value::int(15),
            Value::int(80),
        ],
    )?;
    Ok(Value::none())
}

/// A name from `get_names()`, which the source slices (`name[:5]`).
fn name_text(interp: &mut Interpreter, name: &Value) -> Result<String> {
    if let Some(text) = name.as_str() {
        return Ok(text.to_string());
    }
    let ty = builtin(interp, "type", std::slice::from_ref(name))?;
    let ty = interp.get_attr(&ty, "__name__")?;
    Err(type_error(format!(
        "'{}' object is not subscriptable",
        ty.as_str().unwrap_or("object")
    )))
}

/// `Cmd.print_topics(self, header, cmds, cmdlen, maxcol)`.
fn print_topics(
    interp: &mut Interpreter,
    this: &Value,
    header: &Value,
    cmds: &Value,
    maxcol: &Value,
) -> Result<Value> {
    if truthy(interp, cmds)? {
        let text = to_str(interp, header.clone())?;
        write_out(interp, this, Value::string(format!("{text}\n")))?;
        let ruler = interp.get_attr(this, "ruler")?;
        if truthy(interp, &ruler)? {
            let ruler = interp.get_attr(this, "ruler")?;
            let length = builtin(interp, "len", std::slice::from_ref(header))?;
            let line = binary(interp, &ruler, BinaryOp::Mul, &length)?;
            let line = to_str(interp, line)?;
            write_out(interp, this, Value::string(format!("{line}\n")))?;
        }
        let width = binary(interp, maxcol, BinaryOp::Sub, &Value::int(1))?;
        call_method(interp, this, "columnize", &[cmds.clone(), width])?;
        write_out(interp, this, Value::string("\n"))?;
    }
    Ok(Value::none())
}

/// `isinstance(value, str)`.
fn is_str(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if value.as_str().is_some() {
        return Ok(true);
    }
    let builtins = interp.load_module("builtins")?;
    let str_type = interp.get_attr(&builtins, "str")?;
    let result = builtin(interp, "isinstance", &[value.clone(), str_type])?;
    truthy(interp, &result)
}

/// `Cmd.columnize(self, list, displaywidth=80)`.
fn columnize(
    interp: &mut Interpreter,
    this: &Value,
    list: &Value,
    displaywidth: &Value,
) -> Result<Value> {
    if !truthy(interp, list)? {
        write_out(interp, this, Value::string("<empty>\n"))?;
        return Ok(Value::none());
    }
    let size_value = builtin(interp, "len", std::slice::from_ref(list))?;
    let size = size_value
        .as_int()
        .ok_or_else(|| type_error("'len' must return an int"))?
        .max(0) as usize;
    let mut items = Vec::with_capacity(size);
    for i in 0..size {
        items.push(call_method(
            interp,
            list,
            "__getitem__",
            &[Value::int(i as i64)],
        )?);
    }
    let mut nonstrings = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if !is_str(interp, item)? {
            nonstrings.push(i.to_string());
        }
    }
    if !nonstrings.is_empty() {
        return Err(type_error(format!(
            "list[i] not a string for i in {}",
            nonstrings.join(", ")
        )));
    }
    let texts: Vec<String> = items
        .iter()
        .map(|item| to_str(interp, item.clone()))
        .collect::<Result<_>>()?;
    let lengths: Vec<usize> = texts.iter().map(|t| t.chars().count()).collect();
    if size == 1 {
        let text = percent_format(interp, "%s\n", Value::string(texts[0].as_str()))?;
        write_out(interp, this, text)?;
        return Ok(Value::none());
    }
    // Try every row count from 1 upwards
    let mut layout: Option<(usize, usize, Vec<usize>)> = None;
    for nrows in 1..size {
        let ncols = size.div_ceil(nrows);
        let mut colwidths: Vec<usize> = Vec::new();
        let mut totwidth: i64 = -2;
        for col in 0..ncols {
            let mut colwidth = 0usize;
            for row in 0..nrows {
                let i = row + nrows * col;
                if i >= size {
                    break;
                }
                colwidth = colwidth.max(lengths[i]);
            }
            colwidths.push(colwidth);
            totwidth += colwidth as i64 + 2;
            if compare(interp, &Value::int(totwidth), BinaryOp::Gt, displaywidth)? {
                break;
            }
        }
        if compare(interp, &Value::int(totwidth), BinaryOp::Le, displaywidth)? {
            layout = Some((nrows, ncols, colwidths));
            break;
        }
    }
    let (nrows, ncols, colwidths) = layout.unwrap_or((size, 1, vec![0]));
    for row in 0..nrows {
        let mut line: Vec<&str> = Vec::with_capacity(ncols);
        for col in 0..ncols {
            let i = row + nrows * col;
            line.push(if i >= size { "" } else { texts[i].as_str() });
        }
        while line.last().is_some_and(|t| t.is_empty()) {
            line.pop();
        }
        let padded: Vec<String> = line
            .iter()
            .enumerate()
            .map(|(col, text)| {
                let width = colwidths.get(col).copied().unwrap_or(0);
                let len = text.chars().count();
                let mut padded = (*text).to_string();
                padded.extend(std::iter::repeat_n(' ', width.saturating_sub(len)));
                padded
            })
            .collect();
        write_out(
            interp,
            this,
            Value::string(format!("{}\n", padded.join("  "))),
        )?;
    }
    Ok(Value::none())
}

/// The CPython docstring of a `Cmd` method, by registry name
/// (`cmd.Cmd.do_help`), for `Cmd.do_help.__doc__` and, crucially, for the
/// `getattr(self, name).__doc__` test `do_help` uses to sort the documented
/// commands from the undocumented ones.  `None`: not a `Cmd` method;
/// `Some(None)`: a `Cmd` method that has no docstring (`completenames`, ...).
pub(crate) fn native_doc(registry_key: &str) -> Option<Option<&'static str>> {
    let method = registry_key.strip_prefix("cmd.Cmd.")?;
    Some(Some(match method {
        "__init__" => docs::DOC_INIT,
        "cmdloop" => docs::DOC_CMDLOOP,
        "precmd" => docs::DOC_PRECMD,
        "postcmd" => docs::DOC_POSTCMD,
        "preloop" => docs::DOC_PRELOOP,
        "postloop" => docs::DOC_POSTLOOP,
        "parseline" => docs::DOC_PARSELINE,
        "onecmd" => docs::DOC_ONECMD,
        "emptyline" => docs::DOC_EMPTYLINE,
        "default" => docs::DOC_DEFAULT,
        "completedefault" => docs::DOC_COMPLETEDEFAULT,
        "complete" => docs::DOC_COMPLETE,
        "do_help" => docs::DOC_DO_HELP,
        "columnize" => docs::DOC_COLUMNIZE,
        _ => return Some(None),
    }))
}

/// Wire what the CPython source gets from `import string, sys`, so `cmd.sys`
/// and `cmd.string` exist and can be patched.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    for name in ["string", "sys"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_list(),
        "PROMPT" => Value::string(PROMPT),
        "IDENTCHARS" => Value::string(IDENTCHARS),
    }

    class Cmd {
        constants {
            "__doc__" => Value::string(docs::CLASS_DOC),
            "prompt" => Value::string(PROMPT),
            "identchars" => Value::string(IDENTCHARS),
            "ruler" => Value::string("="),
            "lastcmd" => Value::string(""),
            "intro" => Value::none(),
            "doc_leader" => Value::string(""),
            "doc_header" => Value::string("Documented commands (type help <topic>):"),
            "misc_header" => Value::string("Miscellaneous help topics:"),
            "undoc_header" => Value::string("Undocumented commands:"),
            "nohelp" => Value::string("*** No help on %s"),
            "use_rawinput" => Value::int(1),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_INIT, args)?;
            let this = b.take_or_none(0);
            let completekey = b.take_or(1, || Value::string("tab"));
            let stdin = b.take_or_none(2);
            let stdout = b.take_or_none(3);
            cmd_init(_interp, &this, completekey, stdin, stdout)?;
            Ok(Value::none())
        }

        fn cmdloop(args) -> Result<Value> {
            let mut b = args::bind(&SIG_CMDLOOP, args)?;
            let this = b.take_or_none(0);
            let intro = b.take_or_none(1);
            cmdloop(_interp, &this, &intro)?;
            Ok(Value::none())
        }

        fn precmd(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PRECMD, args)?;
            let _this = b.take_or_none(0);
            Ok(b.take_or_none(1))
        }

        fn postcmd(args) -> Result<Value> {
            let mut b = args::bind(&SIG_POSTCMD, args)?;
            let _this = b.take_or_none(0);
            Ok(b.take_or_none(1))
        }

        fn preloop(args) -> Result<Value> {
            let _ = args::bind(&SIG_PRELOOP, args)?;
            Ok(Value::none())
        }

        fn postloop(args) -> Result<Value> {
            let _ = args::bind(&SIG_POSTLOOP, args)?;
            Ok(Value::none())
        }

        fn parseline(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PARSELINE, args)?;
            let this = b.take_or_none(0);
            let line = b.take_or_none(1);
            parseline(_interp, &this, &line)
        }

        fn onecmd(args) -> Result<Value> {
            let mut b = args::bind(&SIG_ONECMD, args)?;
            let this = b.take_or_none(0);
            let line = b.take_or_none(1);
            onecmd(_interp, &this, &line)
        }

        fn emptyline(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EMPTYLINE, args)?;
            let this = b.take_or_none(0);
            emptyline(_interp, &this)
        }

        fn default(args) -> Result<Value> {
            let mut b = args::bind(&SIG_DEFAULT, args)?;
            let this = b.take_or_none(0);
            let line = b.take_or_none(1);
            default_cmd(_interp, &this, line)
        }

        fn completedefault(args) -> Result<Value> {
            let _ = args::bind(&SIG_COMPLETEDEFAULT, &drop_varargs(args, 1))?;
            Ok(Value::list(Vec::new()))
        }

        fn completenames(args) -> Result<Value> {
            let mut b = args::bind(&SIG_COMPLETENAMES, &drop_varargs(args, 2))?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            completenames(_interp, &this, &text)
        }

        fn complete(args) -> Result<Value> {
            let mut b = args::bind(&SIG_COMPLETE, args)?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            let state = b.take_or_none(2);
            complete(_interp, &this, text, state)
        }

        fn get_names(args) -> Result<Value> {
            let mut b = args::bind(&SIG_GET_NAMES, args)?;
            let this = b.take_or_none(0);
            get_names(_interp, &this)
        }

        fn complete_help(args) -> Result<Value> {
            let mut b = args::bind(&SIG_COMPLETE_HELP, &drop_varargs(args, 1))?;
            let this = b.take_or_none(0);
            let rest = positionals(args);
            complete_help(_interp, &this, rest.get(1..).unwrap_or(&[]))
        }

        fn do_help(args) -> Result<Value> {
            let mut b = args::bind(&SIG_DO_HELP, args)?;
            let this = b.take_or_none(0);
            let arg = b.take_or_none(1);
            do_help(_interp, &this, &arg)
        }

        fn print_topics(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PRINT_TOPICS, args)?;
            let this = b.take_or_none(0);
            let header = b.take_or_none(1);
            let cmds = b.take_or_none(2);
            let _cmdlen = b.take_or_none(3);
            let maxcol = b.take_or_none(4);
            print_topics(_interp, &this, &header, &cmds, &maxcol)
        }

        fn columnize(args) -> Result<Value> {
            let mut b = args::bind(&SIG_COLUMNIZE, args)?;
            let this = b.take_or_none(0);
            let list = b.take_or_none(1);
            let displaywidth = b.take_or(2, || Value::int(80));
            columnize(_interp, &this, &list, &displaywidth)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::interpreter::Interpreter;

    #[test]
    fn cmdloop_dispatches_through_subclass_hooks() {
        let mut interp = Interpreter::default();
        let src = r#"
import cmd, io

class Shell(cmd.Cmd):
    use_rawinput = False
    prompt = "> "

    def do_hello(self, arg):
        "Say hello."
        self.stdout.write("hello %s\n" % arg)

    def do_quit(self, arg):
        return True

out = io.StringIO()
shell = Shell(stdin=io.StringIO("hello world\nhelp\nquit\n"), stdout=out)
shell.cmdloop()
text = out.getvalue()
assert "hello world\n" in text, text
assert "hello  help\n" in text and "Undocumented commands:" in text, text
"#;
        interp
            .exec_source(src, None, None)
            .expect("cmdloop should dispatch to subclass methods");
    }

    #[test]
    fn columnize_matches_cpython_layout() {
        let mut interp = Interpreter::default();
        let src = r#"
import cmd, io

out = io.StringIO()
c = cmd.Cmd(stdout=out)
c.columnize(["a", "bb", "ccc", "dddd"], 8)
assert out.getvalue() == "a   ccc \nbb  dddd\n", repr(out.getvalue())
"#;
        interp
            .exec_source(src, None, None)
            .expect("columnize should lay out columns like CPython");
    }
}
