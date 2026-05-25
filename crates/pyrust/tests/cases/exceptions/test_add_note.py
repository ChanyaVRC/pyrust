# BaseException.add_note() — Python 3.11+ feature (issue #1067).

# --- basic append ---
e = ValueError("test")
e.add_note("note1")
print(e.__notes__)          # ['note1']
e.add_note("note2")
print(e.__notes__)          # ['note1', 'note2']

# --- hasattr is False on a fresh exception ---
e2 = ValueError("no notes")
print(hasattr(e2, "__notes__"))   # False

# --- returns None ---
result = e.add_note("note3")
print(result is None)       # True

# --- TypeError on non-str ---
try:
    e.add_note(42)
except TypeError as te:
    print(te)               # note must be a str, not 'int'

try:
    e.add_note([1, 2, 3])
except TypeError as te:
    print(te)               # note must be a str, not 'list'

# --- arity errors ---
try:
    e.add_note()
except TypeError as te:
    print(te)               # BaseException.add_note() takes exactly one argument (0 given)

try:
    e.add_note("a", "b")
except TypeError as te:
    print(te)               # BaseException.add_note() takes exactly one argument (2 given)

# --- works on BaseException directly ---
be = BaseException("base")
be.add_note("base note")
print(be.__notes__)         # ['base note']

# --- inherited by user-defined subclass ---
class MyError(RuntimeError):
    pass

me = MyError("custom")
me.add_note("custom note")
print(me.__notes__)         # ['custom note']

# --- __notes__ is a real mutable list ---
e3 = ValueError("mutable")
e3.add_note("first")
notes = e3.__notes__
print(type(notes).__name__) # list
notes.append("appended directly")
print(e3.__notes__)         # ['first', 'appended directly']

# --- raise / re-raise scenario ---
try:
    try:
        raise ValueError("main error")
    except ValueError as exc:
        exc.add_note("Additional context note")
        exc.add_note("Another note")
        raise
except ValueError as exc:
    print(exc)              # main error
    print(exc.__notes__)    # ['Additional context note', 'Another note']
