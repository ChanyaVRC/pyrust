# Parity fixture for issue #912: `from module import nonexistent` must raise
# ImportError (not AttributeError) matching CPython 3.12 behaviour.
#
# CPython: ImportError: cannot import name 'x' from 'module' (filename)
# pyrust:  ImportError: cannot import name 'x' from 'module'
# (The trailing filename hint is intentionally omitted; we only check the
#  exception type and the stable prefix of the message.)

# --- Nonexistent name raises ImportError, not AttributeError ---
try:
    from os import nonexistent_symbol
except ImportError as e:
    msg = str(e)
    # Strip any trailing " (/path/to/module.py)" that CPython appends so the
    # output is stable across environments.
    if ' (' in msg:
        msg = msg[:msg.index(' (')]
    print(msg)
except AttributeError as e:
    print("WRONG: AttributeError:", e)

# --- except AttributeError does NOT catch the exception ---
caught_attr = False
try:
    from os import another_nonexistent
except AttributeError:
    caught_attr = True
except ImportError:
    pass
print("caught_attr_error:", caught_attr)

# --- Valid import is unaffected ---
from os import path
print("valid import ok:", path is not None)

# --- Mixed: first name valid, second nonexistent ---
try:
    from os import getcwd, nonexistent2
except ImportError as e:
    msg = str(e)
    if ' (' in msg:
        msg = msg[:msg.index(' (')]
    print(msg)
except AttributeError as e:
    print("WRONG: AttributeError:", e)

# --- Message contains the expected substrings ---
try:
    from os import missing_name
except ImportError as e:
    msg = str(e)
    print("has cannot import name:", "cannot import name" in msg)
    print("has missing_name:", "missing_name" in msg)
    print("has from os:", "from 'os'" in msg)
