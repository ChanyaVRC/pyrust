# Parity test for dotted value patterns in match/case (PEP 634).
# A dotted name like `Color.RED` is a value pattern: the expression is
# evaluated and compared with ==, not bound as a capture variable.


# --- Basic dotted value pattern ---

class Color:
    RED = 1
    GREEN = 2
    BLUE = 3


def describe_color(c):
    match c:
        case Color.RED:
            return "red"
        case Color.GREEN:
            return "green"
        case Color.BLUE:
            return "blue"
        case _:
            return "unknown"


assert describe_color(Color.RED) == "red", f"got {describe_color(Color.RED)}"
assert describe_color(Color.GREEN) == "green", f"got {describe_color(Color.GREEN)}"
assert describe_color(Color.BLUE) == "blue", f"got {describe_color(Color.BLUE)}"
assert describe_color(99) == "unknown", f"got {describe_color(99)}"


# --- Multi-level dotted path ---

class http:
    class client:
        OK = 200
        NOT_FOUND = 404
        SERVER_ERROR = 500


def describe_status(code):
    match code:
        case http.client.OK:
            return "ok"
        case http.client.NOT_FOUND:
            return "not found"
        case http.client.SERVER_ERROR:
            return "server error"
        case _:
            return "other"


assert describe_status(200) == "ok"
assert describe_status(404) == "not found"
assert describe_status(500) == "server error"
assert describe_status(301) == "other"


# --- OR pattern combining dotted values ---

class Status:
    PENDING = 0
    RUNNING = 1
    DONE = 2
    ERROR = 3


def is_terminal(s):
    match s:
        case Status.DONE | Status.ERROR:
            return True
        case _:
            return False


assert is_terminal(Status.DONE) is True
assert is_terminal(Status.ERROR) is True
assert is_terminal(Status.PENDING) is False
assert is_terminal(Status.RUNNING) is False


# --- Bare name still captures (no regression) ---

match 42:
    case captured_x:
        pass
assert captured_x == 42, f"capture regression: {captured_x}"


# --- Wildcard still works (no regression) ---

matched_wc = False
match 99:
    case _:
        matched_wc = True
assert matched_wc is True


# --- Value comparison uses == (not identity) ---

class Const:
    VAL = 1000  # use a value large enough that it may not be interned


matched_eq = False
match 1000:
    case Const.VAL:
        matched_eq = True
assert matched_eq is True, "value pattern must use == comparison"


# --- No arm matches when value differs ---

no_match = "untouched"
match Color.RED:
    case Color.GREEN:
        no_match = "wrong"
    case Color.BLUE:
        no_match = "wrong"
assert no_match == "untouched", f"unexpected match: {no_match}"


print("match dotted value OK")
