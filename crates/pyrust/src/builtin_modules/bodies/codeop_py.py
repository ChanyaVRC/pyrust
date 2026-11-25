r"""Utilities to compile possibly incomplete Python source code.

This module provides two interfaces, broadly similar to the builtin
function compile(), which take program text, a filename and a 'mode'
and:

- Return code object if the command is complete and valid
- Return None if the command is incomplete
- Raise SyntaxError, ValueError or OverflowError if the command is a
  syntax error (OverflowError and ValueError can be produced by
  malformed literals).

The two interfaces are:

compile_command(source, filename, symbol):

    Compiles a single command in the manner described above.

CommandCompiler():

    Instances of this class have __call__ methods identical in
    signature to compile_command; the difference is that if the
    instance compiles program text containing a __future__ statement,
    the instance 'remembers' and compiles all subsequent program texts
    with the statement in force.

The module also provides another class:

Compile():

    Instances of this class act like the built-in function compile,
    but with 'memory' in the sense described above.
"""

import __future__
import ast
import warnings

_features = [getattr(__future__, fname)
             for fname in __future__.all_feature_names]

__all__ = ["compile_command", "Compile", "CommandCompiler"]

# The following flags match the values from Include/cpython/compile.h
# Caveat emptor: These flags are undocumented on purpose and depending
# on their effect outside the standard library is **unsupported**.
PyCF_DONT_IMPLY_DEDENT = 0x200
PyCF_ALLOW_INCOMPLETE_INPUT = 0x4000

_COMPOUND_NODES = tuple(
    cls for cls in (
        getattr(ast, "If", None),
        getattr(ast, "For", None),
        getattr(ast, "AsyncFor", None),
        getattr(ast, "While", None),
        getattr(ast, "With", None),
        getattr(ast, "AsyncWith", None),
        getattr(ast, "Try", None),
        getattr(ast, "TryStar", None),
        getattr(ast, "FunctionDef", None),
        getattr(ast, "AsyncFunctionDef", None),
        getattr(ast, "ClassDef", None),
        getattr(ast, "Match", None),
    )
    if cls is not None
)


def _is_compound_single(source, filename):
    """Return True if `source` parses as a single compound statement."""
    try:
        tree = ast.parse(source, filename, "exec")
    except SyntaxError:
        return False
    return len(tree.body) == 1 and isinstance(tree.body[0], _COMPOUND_NODES)


def _collect_indents(source):
    """Collect distinct leading-whitespace indents of non-blank, non-comment lines."""
    last_indent = ""
    seen = set()
    ordered = []
    for raw in source.split("\n"):
        stripped = raw.lstrip(" \t")
        if not stripped or stripped[0] == "#":
            continue
        indent = raw[:len(raw) - len(stripped)]
        last_indent = indent
        if indent not in seen:
            seen.add(indent)
            ordered.append(indent)
    if "" not in seen:
        ordered.append("")
    ordered.sort(key=len, reverse=True)
    return last_indent, ordered


def _try_compile(compiler, candidate, filename, symbol):
    try:
        compiler(candidate, filename, symbol)
        return True
    except SyntaxError:
        return False


def _has_trailing_line_continuation(source):
    """Return True if `source` ends with an odd run of backslashes (no trailing newline)."""
    if not source or source.endswith(("\n", "\r")):
        return False
    count = 0
    idx = len(source) - 1
    while idx >= 0 and source[idx] == "\\":
        count += 1
        idx -= 1
    return (count % 2) == 1


def _is_incomplete(compiler, source, filename, symbol, orig_err, depth=0):
    """Return True when `orig_err` indicates incomplete input rather than a syntax error."""
    # Classic two-pass newline probing: handles single-line compound statements
    # such as `if True: pass` or `def f(): return 1` that need a trailing newline.
    if _try_compile(compiler, source + "\n", filename, symbol):
        return True
    if _try_compile(compiler, source + "\n\n", filename, symbol):
        return True

    msg = str(orig_err)
    if "incomplete input" in msg:
        return True

    # Trailing line continuation backslash (`1 + \`, `x = \`, etc.).
    if _has_trailing_line_continuation(source):
        for suffix in ("\n0", "\npass", "\npass\n", "\nf(): pass\n"):
            if _try_compile(compiler, source + suffix, filename, symbol):
                return True

    # Unterminated triple-quoted string or bytes literal (`"""hello`, `'''hello`, `r"""...`, `b"""...`).
    if depth == 0 and "unterminated triple-quoted" in msg:
        for quote in ('"""', "'''"):
            for pad in ("", " "):
                closed = source + pad + quote
                try:
                    compiler(closed, filename, symbol)
                    return True
                except SyntaxError as closed_err:
                    if _is_incomplete(
                        compiler, closed, filename, symbol, closed_err, depth=1
                    ):
                        return True
        return False

    # Unterminated f-string expression (`f"{1 +`, `f"""{1 +`).
    if "unterminated f-string expression" in msg:
        for suffix in (
            '0}"', "0}'", '0}"""', "0}'''",
            '}"', "}'", '}"""', "}'''",
        ):
            if _try_compile(compiler, source + suffix, filename, symbol):
                return True
        return False

    # Unclosed opening delimiter `(`, `[`, `{` reached EOF cleanly.
    if "was never closed" in msg:
        return True

    if symbol != "eval":
        # Open parameter/base list reaching EOF (`def f(a,\n`, `class C(A,`).
        if "Some(Eof)" in msg:
            for suffix in ("): pass\n", "):\n    pass\n", ")\n"):
                if _try_compile(compiler, source + suffix, filename, symbol):
                    return True

        # Incomplete compound statements, suites, `try`, `match`, or decorators.
        prefix = source if source.endswith("\n") else source + "\n"
        last_indent, indents = _collect_indents(source)

        # Indented suite body (`def f():`, `if True:`, `else:`, `elif:`, `case:`, etc.).
        if _try_compile(compiler, prefix + last_indent + "    pass\n", filename, symbol):
            return True

        # Bare `try:` / `try:\n` awaiting both body and `except`/`finally`.
        if _try_compile(
            compiler,
            prefix + last_indent + "    pass\n" + last_indent + "except:\n" + last_indent + "    pass\n",
            filename,
            symbol,
        ):
            return True

        # `try:\n    pass` awaiting `except`/`finally`, `match x:` awaiting `case`,
        # or `@decorator` awaiting `def`/`class` at any enclosing indentation level.
        for ind in indents:
            if _try_compile(
                compiler,
                prefix + ind + "except:\n" + ind + "    pass\n",
                filename,
                symbol,
            ):
                return True
            if _try_compile(
                compiler,
                prefix + ind + "    case _:\n" + ind + "        pass\n",
                filename,
                symbol,
            ):
                return True
            if _try_compile(
                compiler,
                prefix + ind + "def _(): pass\n",
                filename,
                symbol,
            ):
                return True

    return False


def _maybe_compile(compiler, source, filename, symbol):
    # Check for source consisting of only blank lines and comments.
    for line in source.split("\n"):
        line = line.strip()
        if line and line[0] != '#':
            break               # Leave it alone.
    else:
        if symbol != "eval":
            source = "pass"     # Replace it with a 'pass' statement
        else:
            return None

    # Disable compiler warnings when checking for incomplete input.
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", (SyntaxWarning, DeprecationWarning))
        try:
            compiler(source, filename, symbol)
        except SyntaxError as orig_err:  # Let other compile() errors propagate.
            if _is_incomplete(compiler, source, filename, symbol, orig_err):
                return None
            raise
        else:
            # In interactive ("single") mode, a compound statement requires a
            # trailing newline (empty line in InteractiveConsole) before it is
            # considered complete (matches CPython's PyCF_DONT_IMPLY_DEDENT).
            if (
                symbol == "single"
                and not source.endswith("\n")
                and _is_compound_single(source, filename)
            ):
                return None

    return compiler(source, filename, symbol, incomplete_input=False)


def _is_syntax_error(err1, err2):
    rep1 = repr(err1)
    rep2 = repr(err2)
    if "was never closed" in rep1 and "was never closed" in rep2:
        return False
    if rep1 == rep2:
        return True
    return False


def _compile(source, filename, symbol, incomplete_input=True):
    flags = 0
    if incomplete_input:
        flags |= PyCF_ALLOW_INCOMPLETE_INPUT
        flags |= PyCF_DONT_IMPLY_DEDENT
    return compile(source, filename, symbol, flags)


def compile_command(source, filename="<input>", symbol="single"):
    r"""Compile a command and determine whether it is incomplete.

    Arguments:

    source -- the source string; may contain \n characters
    filename -- optional filename from which source was read; default
                "<input>"
    symbol -- optional grammar start symbol; "single" (default), "exec"
              or "eval"

    Return value / exceptions raised:

    - Return a code object if the command is complete and valid
    - Return None if the command is incomplete
    - Raise SyntaxError, ValueError or OverflowError if the command is a
      syntax error (OverflowError and ValueError can be produced by
      malformed literals).
    """
    return _maybe_compile(_compile, source, filename, symbol)


class Compile:
    """Instances of this class behave much like the built-in compile
    function, but if one is used to compile text containing a future
    statement, it "remembers" and compiles all subsequent program texts
    with the statement in force."""

    def __init__(self):
        self.flags = PyCF_DONT_IMPLY_DEDENT | PyCF_ALLOW_INCOMPLETE_INPUT

    def _record_future_flags(self, source):
        try:
            tree = ast.parse(source, "<future>", "exec")
        except SyntaxError:
            return
        idx = 0
        body = tree.body
        # Allow a leading module docstring before future imports (PEP 236).
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)
        ):
            idx = 1
        while idx < len(body):
            node = body[idx]
            if (
                isinstance(node, ast.ImportFrom)
                and node.module == "__future__"
                and node.level == 0
            ):
                for alias in node.names:
                    feature = getattr(__future__, alias.name, None)
                    if feature is not None and hasattr(feature, "compiler_flag"):
                        self.flags |= feature.compiler_flag
                idx += 1
            else:
                break

    def __call__(self, source, filename, symbol, **kwargs):
        flags = self.flags
        if kwargs.get('incomplete_input', True) is False:
            flags &= ~PyCF_DONT_IMPLY_DEDENT
            flags &= ~PyCF_ALLOW_INCOMPLETE_INPUT
        codeob = compile(source, filename, symbol, flags, True)
        for feature in _features:
            if codeob.co_flags & feature.compiler_flag:
                self.flags |= feature.compiler_flag
        if symbol != "eval" and "__future__" in source:
            self._record_future_flags(source)
        return codeob


class CommandCompiler:
    """Instances of this class have __call__ methods identical in
    signature to compile_command; the difference is that if the
    instance compiles program text containing a __future__ statement,
    the instance 'remembers' and compiles all subsequent program texts
    with the statement in force."""

    def __init__(self):
        self.compiler = Compile()

    def __call__(self, source, filename="<input>", symbol="single"):
        r"""Compile a command and determine whether it is incomplete.

        Arguments:

        source -- the source string; may contain \n characters
        filename -- optional filename from which source was read;
                    default "<input>"
        symbol -- optional grammar start symbol; "single" (default) or
                  "eval"

        Return value / exceptions raised:

        - Return a code object if the command is complete and valid
        - Return None if the command is incomplete
        - Raise SyntaxError, ValueError or OverflowError if the command is a
          syntax error (OverflowError and ValueError can be produced by
          malformed literals).
        """
        return _maybe_compile(self.compiler, source, filename, symbol)
