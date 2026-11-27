// `netrc` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/netrc.py` (issues #3230, #3451).
//
// The module mirrors the CPython source structure: `NetrcParseError`, the
// private `_netrclex` tokenizer and the `netrc` class whose `__init__`,
// `_parse`, `_security_check`, `authenticators` and `__repr__` are the native
// counterparts of the Python methods.  Like the Python source, the parser
// reaches the file object, the lexer and the other methods through ordinary
// attribute lookups (`self._parse`, `lexer.get_token()`, `fp.read(1)`,
// `os.fstat`, ...), so subclasses, monkeypatching of `netrc.os` and
// non-file-like streams behave exactly as in CPython.
//
// Reference: <https://docs.python.org/3.12/library/netrc.html>

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

const MODULE_DOC: &str = "An object-oriented interface to .netrc files.";

// `def`s of `Lib/netrc.py`: CPython reports arity errors for these in the
// interpreter's `initialize_locals` wording, not the macro's.
const SIG_PARSE_ERROR_INIT: Sig = Sig {
    qualname: "NetrcParseError.__init__",
    params: &["self", "msg", "filename", "lineno"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_PARSE_ERROR_STR: Sig = Sig {
    qualname: "NetrcParseError.__str__",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_LEX_INIT: Sig = Sig {
    qualname: "_netrclex.__init__",
    params: &["self", "fp"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_READ_CHAR: Sig = Sig {
    qualname: "_netrclex._read_char",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_GET_TOKEN: Sig = Sig {
    qualname: "_netrclex.get_token",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_PUSH_TOKEN: Sig = Sig {
    qualname: "_netrclex.push_token",
    params: &["self", "token"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_NETRC_INIT: Sig = Sig {
    qualname: "netrc.__init__",
    params: &["self", "file"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_PARSE: Sig = Sig {
    qualname: "netrc._parse",
    params: &["self", "file", "fp", "default_netrc"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_SECURITY_CHECK: Sig = Sig {
    qualname: "netrc._security_check",
    params: &["self", "fp", "default_netrc", "login"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_AUTHENTICATORS: Sig = Sig {
    qualname: "netrc.authenticators",
    params: &["self", "host"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_REPR: Sig = Sig {
    qualname: "netrc.__repr__",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

fn all_list() -> Value {
    Value::list(
        ["netrc", "NetrcParseError"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

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

/// `fmt % value` with CPython's `%` formatting semantics.
fn percent_format(interp: &mut Interpreter, fmt: &str, value: Value) -> Result<Value> {
    interp.eval_binary(Value::string(fmt), BinaryOp::Mod, value)
}

/// `str(value)`.
fn to_str(interp: &mut Interpreter, value: Value) -> Result<String> {
    let text = percent_format(interp, "%s", Value::tuple(vec![value]))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

/// A token / character produced by the lexer, which CPython's `+=` on a `str`
/// requires to be a `str`.
fn expect_str(value: &Value) -> Result<String> {
    match value.as_str() {
        Some(s) => Ok(s.to_string()),
        None => Err(type_error("can only concatenate str to str")),
    }
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

fn module_value(interp: &mut Interpreter) -> Result<Value> {
    interp.load_module("netrc")
}

/// A module-level dependency (`os`, `stat`): the module attribute when present
/// (CPython resolves `import os` as a module global), otherwise the imported
/// module itself.
fn dep(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = module_value(interp)?;
    match interp.get_attr(&module, name) {
        Ok(value) => Ok(value),
        Err(_) => interp.load_module(name),
    }
}

/// `raise NetrcParseError(msg, filename, lineno)`.
fn parse_error(
    interp: &mut Interpreter,
    msg: Value,
    filename: Option<Value>,
    lineno: Option<Value>,
) -> PyError {
    let build = |interp: &mut Interpreter| -> Result<Value> {
        let module = module_value(interp)?;
        let class = interp.get_attr(&module, "NetrcParseError")?;
        let mut call_args = vec![msg.clone()];
        if let Some(filename) = filename.clone() {
            call_args.push(filename);
            if let Some(lineno) = lineno.clone() {
                call_args.push(lineno);
            }
        }
        call_pos(interp, class, &call_args)
    };
    match build(interp) {
        Ok(instance) => PyError::raised(instance),
        Err(err) => err,
    }
}

// ---------------------------------------------------------------------------
// `_netrclex`
// ---------------------------------------------------------------------------

fn lexer_lineno(interp: &mut Interpreter, lexer: &Value) -> Result<Value> {
    interp.get_attr(lexer, "lineno")
}

fn lineno_i64(value: &Value) -> Result<i64> {
    value
        .as_int()
        .ok_or_else(|| type_error("'lineno' must be an int"))
}

/// `ch in self.whitespace`.
fn is_whitespace(interp: &mut Interpreter, whitespace: &Value, ch: &str) -> Result<bool> {
    if let Some(ws) = whitespace.as_str() {
        return Ok(ws.contains(ch));
    }
    let found = call_method(interp, whitespace, "__contains__", &[Value::string(ch)])?;
    interp.truthy_value(&found)
}

/// `self._read_char()` through the already looked-up bound method; the result
/// is the raw character string (empty at EOF).
fn read_char_raw(interp: &mut Interpreter, read_char: &Value) -> Result<String> {
    let ch = call_pos(interp, read_char.clone(), &[])?;
    expect_str(&ch)
}

/// One step of `iter(self._read_char, "")`: `None` once the sentinel `""` has
/// been produced, after which the callable iterator never calls again.
fn fiter_next(
    interp: &mut Interpreter,
    read_char: &Value,
    exhausted: &mut bool,
) -> Result<Option<String>> {
    if *exhausted {
        return Ok(None);
    }
    let ch = read_char_raw(interp, read_char)?;
    if ch.is_empty() {
        *exhausted = true;
        return Ok(None);
    }
    Ok(Some(ch))
}

fn lex_read_char(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let instream = interp.get_attr(this, "instream")?;
    let ch = call_method(interp, &instream, "read", &[Value::int(1)])?;
    if ch.as_str() == Some("\n") {
        let lineno = lexer_lineno(interp, this)?;
        let next = interp.eval_binary(lineno, BinaryOp::Add, Value::int(1))?;
        interp.assign_attr(this.clone(), "lineno", next)?;
    }
    Ok(ch)
}

fn lex_get_token(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let pushback = interp.get_attr(this, "pushback")?;
    if interp.truthy_value(&pushback)? {
        return call_method(interp, &pushback, "pop", &[Value::int(0)]);
    }
    let mut token = String::new();
    let read_char = interp.get_attr(this, "_read_char")?;
    let mut exhausted = false;
    while let Some(ch) = fiter_next(interp, &read_char, &mut exhausted)? {
        let whitespace = interp.get_attr(this, "whitespace")?;
        if is_whitespace(interp, &whitespace, &ch)? {
            continue;
        }
        if ch == "\"" {
            while let Some(mut ch) = fiter_next(interp, &read_char, &mut exhausted)? {
                if ch == "\"" {
                    return Ok(Value::string(token));
                } else if ch == "\\" {
                    ch = read_char_raw(interp, &read_char)?;
                }
                token.push_str(&ch);
            }
        } else {
            let mut ch = ch;
            if ch == "\\" {
                ch = read_char_raw(interp, &read_char)?;
            }
            token.push_str(&ch);
            while let Some(mut ch) = fiter_next(interp, &read_char, &mut exhausted)? {
                let whitespace = interp.get_attr(this, "whitespace")?;
                if is_whitespace(interp, &whitespace, &ch)? {
                    return Ok(Value::string(token));
                } else if ch == "\\" {
                    ch = read_char_raw(interp, &read_char)?;
                }
                token.push_str(&ch);
            }
        }
    }
    Ok(Value::string(token))
}

// ---------------------------------------------------------------------------
// `netrc`
// ---------------------------------------------------------------------------

/// `self.<attr>[key] = value` for the `hosts` / `macros` dicts.
fn store_item(
    interp: &mut Interpreter,
    this: &Value,
    attr: &str,
    key: &Value,
    value: Value,
) -> Result<()> {
    let table = interp.get_attr(this, attr)?;
    call_method(interp, &table, "__setitem__", &[key.clone(), value])?;
    Ok(())
}

/// `lexer.instream.readline()`.
fn instream_readline(interp: &mut Interpreter, lexer: &Value) -> Result<String> {
    let instream = interp.get_attr(lexer, "instream")?;
    let line = call_method(interp, &instream, "readline", &[])?;
    expect_str(&line)
}

fn netrc_parse(
    interp: &mut Interpreter,
    this: &Value,
    file: &Value,
    fp: &Value,
    default_netrc: &Value,
) -> Result<()> {
    let module = module_value(interp)?;
    let lexer_class = interp.get_attr(&module, "_netrclex")?;
    let lexer = call_pos(interp, lexer_class, std::slice::from_ref(fp))?;
    loop {
        // Look for a machine, default, or macdef top-level keyword
        let saved_lineno = lineno_i64(&lexer_lineno(interp, &lexer)?)?;
        let tt_value = call_method(interp, &lexer, "get_token", &[])?;
        let tt = expect_str(&tt_value)?;
        let entryname_value = if tt.is_empty() {
            break;
        } else if tt.starts_with('#') {
            let lineno = lineno_i64(&lexer_lineno(interp, &lexer)?)?;
            if lineno == saved_lineno && tt.chars().count() == 1 {
                instream_readline(interp, &lexer)?;
            }
            continue;
        } else if tt == "machine" {
            call_method(interp, &lexer, "get_token", &[])?
        } else if tt == "default" {
            Value::string("default")
        } else if tt == "macdef" {
            let entryname = call_method(interp, &lexer, "get_token", &[])?;
            let lines = Value::list(Vec::new());
            store_item(interp, this, "macros", &entryname, lines.clone())?;
            loop {
                let line = instream_readline(interp, &lexer)?;
                if line.is_empty() {
                    let lineno = lexer_lineno(interp, &lexer)?;
                    return Err(parse_error(
                        interp,
                        Value::string("Macro definition missing null line terminator."),
                        Some(file.clone()),
                        Some(lineno),
                    ));
                }
                if line == "\n" {
                    // a macro definition finished with consecutive new-line
                    // characters. The first \n is encountered by the
                    // readline() method and this is the second \n.
                    break;
                }
                call_method(interp, &lines, "append", &[Value::string(line)])?;
            }
            continue;
        } else {
            let msg = percent_format(interp, "bad toplevel token %r", tt_value.clone())?;
            let lineno = lexer_lineno(interp, &lexer)?;
            return Err(parse_error(interp, msg, Some(file.clone()), Some(lineno)));
        };

        if !interp.truthy_value(&entryname_value)? {
            let msg = percent_format(interp, "missing %r name", tt_value.clone())?;
            let lineno = lexer_lineno(interp, &lexer)?;
            return Err(parse_error(interp, msg, Some(file.clone()), Some(lineno)));
        }

        // We're looking at start of an entry for a named machine or default.
        let mut login = Value::string("");
        let mut account = Value::string("");
        let mut password = Value::string("");
        store_item(
            interp,
            this,
            "hosts",
            &entryname_value,
            Value::dict(crate::value::PyDict::default()),
        )?;
        loop {
            let prev_lineno = lineno_i64(&lexer_lineno(interp, &lexer)?)?;
            let tt_value = call_method(interp, &lexer, "get_token", &[])?;
            let tt = expect_str(&tt_value)?;
            if tt.starts_with('#') {
                let lineno = lineno_i64(&lexer_lineno(interp, &lexer)?)?;
                if lineno == prev_lineno {
                    instream_readline(interp, &lexer)?;
                }
                continue;
            }
            if matches!(tt.as_str(), "" | "machine" | "default" | "macdef") {
                store_item(
                    interp,
                    this,
                    "hosts",
                    &entryname_value,
                    Value::tuple(vec![login.clone(), account.clone(), password.clone()]),
                )?;
                call_method(interp, &lexer, "push_token", &[tt_value])?;
                break;
            } else if tt == "login" || tt == "user" {
                login = call_method(interp, &lexer, "get_token", &[])?;
            } else if tt == "account" {
                account = call_method(interp, &lexer, "get_token", &[])?;
            } else if tt == "password" {
                password = call_method(interp, &lexer, "get_token", &[])?;
            } else {
                let msg = percent_format(interp, "bad follower token %r", tt_value.clone())?;
                let lineno = lexer_lineno(interp, &lexer)?;
                return Err(parse_error(interp, msg, Some(file.clone()), Some(lineno)));
            }
        }
        call_method(
            interp,
            this,
            "_security_check",
            &[fp.clone(), default_netrc.clone(), login],
        )?;
    }
    Ok(())
}

/// `pwd.getpwuid(uid)[0]`, or `'uid %s' % uid` when there is no such user.
fn user_name(interp: &mut Interpreter, uid: &Value) -> Result<String> {
    let pwd = interp.load_module("pwd")?;
    let entry = try_swallow(
        || call_method(interp, &pwd, "getpwuid", std::slice::from_ref(uid)),
        |err| err.class_name_is("KeyError"),
    )?;
    match entry {
        Ok(entry) => {
            let name = call_method(interp, &entry, "__getitem__", &[Value::int(0)])?;
            to_str(interp, name)
        }
        Err(_) => {
            let uid_text = to_str(interp, uid.clone())?;
            Ok(format!("uid {uid_text}"))
        }
    }
}

fn netrc_security_check(
    interp: &mut Interpreter,
    fp: &Value,
    default_netrc: &Value,
    login: &Value,
) -> Result<()> {
    let os = dep(interp, "os")?;
    let os_name = interp.get_attr(&os, "name")?;
    if os_name.as_str() != Some("posix")
        || !interp.truthy_value(default_netrc)?
        || login.as_str() == Some("anonymous")
    {
        return Ok(());
    }
    let fileno = call_method(interp, fp, "fileno", &[])?;
    let prop = call_method(interp, &os, "fstat", &[fileno])?;
    let st_uid = interp.get_attr(&prop, "st_uid")?;
    let getuid = call_method(interp, &os, "getuid", &[])?;
    if st_uid.as_int() != getuid.as_int() {
        let fowner = user_name(interp, &st_uid)?;
        let user = user_name(interp, &getuid)?;
        return Err(parse_error(
            interp,
            Value::string(format!(
                "~/.netrc file owner ({fowner}, {user}) does not match current user"
            )),
            None,
            None,
        ));
    }
    let st_mode = interp.get_attr(&prop, "st_mode")?;
    // stat.S_IRWXG | stat.S_IRWXO
    if st_mode.as_int().unwrap_or(0) & 0o077 != 0 {
        return Err(parse_error(
            interp,
            Value::string(
                "~/.netrc access too permissive: access permissions must restrict access to only the owner",
            ),
            None,
            None,
        ));
    }
    Ok(())
}

/// `with open(file, encoding=encoding) as fp: self._parse(file, fp, default_netrc)`
fn parse_with_encoding(
    interp: &mut Interpreter,
    this: &Value,
    file: &Value,
    encoding: &str,
    default_netrc: &Value,
) -> Result<()> {
    let builtins = interp.load_module("builtins")?;
    let open = interp.get_attr(&builtins, "open")?;
    let opened = interp.call_function_expanded(
        open,
        &[
            ExpandedCallArg {
                name: None,
                value: file.clone(),
            },
            ExpandedCallArg {
                name: Some("encoding".to_string()),
                value: Value::string(encoding),
            },
        ],
    )?;
    let fp = call_method(interp, &opened, "__enter__", &[])?;
    let parsed = (|| {
        let parse = interp.get_attr(this, "_parse")?;
        call_pos(
            interp,
            parse,
            &[file.clone(), fp.clone(), default_netrc.clone()],
        )
    })();
    let exit_args = [Value::none(), Value::none(), Value::none()];
    match parsed {
        Ok(_) => {
            call_method(interp, &opened, "__exit__", &exit_args)?;
            Ok(())
        }
        Err(err) => {
            let _ = call_method(interp, &opened, "__exit__", &exit_args);
            Err(err)
        }
    }
}

fn netrc_init(interp: &mut Interpreter, this: &Value, file: Option<Value>) -> Result<()> {
    let default_netrc = file.as_ref().is_none_or(Value::is_none);
    let file = if default_netrc {
        // os.path.join(os.path.expanduser("~"), ".netrc")
        let os = dep(interp, "os")?;
        let path = interp.get_attr(&os, "path")?;
        let home = call_method(interp, &path, "expanduser", &[Value::string("~")])?;
        call_method(interp, &path, "join", &[home, Value::string(".netrc")])?
    } else {
        file.unwrap_or_else(Value::none)
    };
    interp.assign_attr(
        this.clone(),
        "hosts",
        Value::dict(crate::value::PyDict::default()),
    )?;
    interp.assign_attr(
        this.clone(),
        "macros",
        Value::dict(crate::value::PyDict::default()),
    )?;
    let default_flag = Value::bool_(default_netrc);
    match parse_with_encoding(interp, this, &file, "utf-8", &default_flag) {
        Err(err) if err.class_name_is("UnicodeDecodeError") => {
            pyrust_core::reset_captured_error_frames();
            parse_with_encoding(interp, this, &file, "locale", &default_flag)
        }
        other => other,
    }
}

fn netrc_authenticators(interp: &mut Interpreter, this: &Value, host: &Value) -> Result<Value> {
    let hosts = interp.get_attr(this, "hosts")?;
    let found = call_method(interp, &hosts, "__contains__", std::slice::from_ref(host))?;
    if interp.truthy_value(&found)? {
        let hosts = interp.get_attr(this, "hosts")?;
        return call_method(interp, &hosts, "__getitem__", std::slice::from_ref(host));
    }
    let hosts = interp.get_attr(this, "hosts")?;
    let found = call_method(interp, &hosts, "__contains__", &[Value::string("default")])?;
    if interp.truthy_value(&found)? {
        let hosts = interp.get_attr(this, "hosts")?;
        return call_method(interp, &hosts, "__getitem__", &[Value::string("default")]);
    }
    Ok(Value::none())
}

fn item(interp: &mut Interpreter, target: &Value, index: i64) -> Result<Value> {
    call_method(interp, target, "__getitem__", &[Value::int(index)])
}

/// Dump the class data in the format of a .netrc file.
fn netrc_repr(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let mut rep = String::new();
    let hosts = interp.get_attr(this, "hosts")?;
    let keys = call_method(interp, &hosts, "keys", &[])?;
    for host in interp.collect_iterable(&keys)? {
        let hosts = interp.get_attr(this, "hosts")?;
        let attrs = call_method(interp, &hosts, "__getitem__", std::slice::from_ref(&host))?;
        let host_text = to_str(interp, host)?;
        let login = item(interp, &attrs, 0)?;
        let login_text = to_str(interp, login)?;
        rep.push_str(&format!("machine {host_text}\n\tlogin {login_text}\n"));
        let account = item(interp, &attrs, 1)?;
        if interp.truthy_value(&account)? {
            let account_text = to_str(interp, account)?;
            rep.push_str(&format!("\taccount {account_text}\n"));
        }
        let password = item(interp, &attrs, 2)?;
        let password_text = to_str(interp, password)?;
        rep.push_str(&format!("\tpassword {password_text}\n"));
    }
    let macros = interp.get_attr(this, "macros")?;
    let keys = call_method(interp, &macros, "keys", &[])?;
    for macro_name in interp.collect_iterable(&keys)? {
        let macros = interp.get_attr(this, "macros")?;
        let lines = call_method(
            interp,
            &macros,
            "__getitem__",
            std::slice::from_ref(&macro_name),
        )?;
        let name_text = to_str(interp, macro_name)?;
        rep.push_str(&format!("macdef {name_text}\n"));
        for line in interp.collect_iterable(&lines)? {
            rep.push_str(&expect_str(&line)?);
        }
        rep.push('\n');
    }
    Ok(Value::string(rep))
}

/// Wire what the CPython source gets from `import os, stat`, so `netrc.os`
/// and `netrc.stat` exist and can be patched.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    for name in ["os", "stat"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
    }

    class NetrcParseError("Exception") {
        constants {
            "__doc__" => Value::string("Exception raised on syntax errors in the .netrc file."),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PARSE_ERROR_INIT, args)?;
            let this = b.take_or_none(0);
            let msg = b.take_or_none(1);
            let filename = b.take_or_none(2);
            let lineno = b.take_or_none(3);
            _interp.assign_attr(this.clone(), "filename", filename)?;
            _interp.assign_attr(this.clone(), "lineno", lineno)?;
            _interp.assign_attr(this.clone(), "msg", msg.clone())?;
            // `Exception.__init__(self, msg)`
            _interp.assign_attr(this, "args", Value::tuple(vec![msg]))?;
            Ok(Value::none())
        }

        fn __str__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PARSE_ERROR_STR, args)?;
            let this = b.take_or_none(0);
            let msg = _interp.get_attr(&this, "msg")?;
            let filename = _interp.get_attr(&this, "filename")?;
            let lineno = _interp.get_attr(&this, "lineno")?;
            percent_format(
                _interp,
                "%s (%s, line %s)",
                Value::tuple(vec![msg, filename, lineno]),
            )
        }
    }

    class _netrclex {
        constants {
            "__doc__" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_LEX_INIT, args)?;
            let this = b.take_or_none(0);
            let fp = b.take_or_none(1);
            _interp.assign_attr(this.clone(), "lineno", Value::int(1))?;
            _interp.assign_attr(this.clone(), "instream", fp)?;
            _interp.assign_attr(this.clone(), "whitespace", Value::string("\n\t\r "))?;
            _interp.assign_attr(this, "pushback", Value::list(Vec::new()))?;
            Ok(Value::none())
        }

        fn _read_char(args) -> Result<Value> {
            let mut b = args::bind(&SIG_READ_CHAR, args)?;
            let this = b.take_or_none(0);
            lex_read_char(_interp, &this)
        }

        fn get_token(args) -> Result<Value> {
            let mut b = args::bind(&SIG_GET_TOKEN, args)?;
            let this = b.take_or_none(0);
            lex_get_token(_interp, &this)
        }

        fn push_token(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PUSH_TOKEN, args)?;
            let this = b.take_or_none(0);
            let token = b.take_or_none(1);
            let pushback = _interp.get_attr(&this, "pushback")?;
            call_method(_interp, &pushback, "append", &[token])?;
            Ok(Value::none())
        }
    }

    class netrc {
        constants {
            "__doc__" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NETRC_INIT, args)?;
            let this = b.take_or_none(0);
            let file = b.take(1);
            netrc_init(_interp, &this, file)?;
            Ok(Value::none())
        }

        fn _parse(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PARSE, args)?;
            let this = b.take_or_none(0);
            let file = b.take_or_none(1);
            let fp = b.take_or_none(2);
            let default_netrc = b.take_or_none(3);
            netrc_parse(_interp, &this, &file, &fp, &default_netrc)?;
            Ok(Value::none())
        }

        fn _security_check(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SECURITY_CHECK, args)?;
            let _this = b.take_or_none(0);
            let fp = b.take_or_none(1);
            let default_netrc = b.take_or_none(2);
            let login = b.take_or_none(3);
            netrc_security_check(_interp, &fp, &default_netrc, &login)?;
            Ok(Value::none())
        }

        /// Return a (user, account, password) tuple for given host.
        fn authenticators(args) -> Result<Value> {
            let mut b = args::bind(&SIG_AUTHENTICATORS, args)?;
            let this = b.take_or_none(0);
            let host = b.take_or_none(1);
            netrc_authenticators(_interp, &this, &host)
        }

        /// Dump the class data in the format of a .netrc file.
        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_REPR, args)?;
            let this = b.take_or_none(0);
            netrc_repr(_interp, &this)
        }
    }
}
