# Parity test for match/case structural pattern matching (PEP 634)

# --- Literal patterns + capture + wildcard + guard ---

def classify(x):
    match x:
        case 0:
            return "zero"
        case 1 | 2 | 3:
            return "small"
        case n if n < 0:
            return "negative"
        case _:
            return "large"

assert classify(0) == "zero", f"got {classify(0)}"
assert classify(1) == "small", f"got {classify(1)}"
assert classify(2) == "small", f"got {classify(2)}"
assert classify(3) == "small", f"got {classify(3)}"
assert classify(-5) == "negative", f"got {classify(-5)}"
assert classify(100) == "large", f"got {classify(100)}"

# --- Capture pattern ---
match 42:
    case x:
        captured = x
assert captured == 42, f"captured={captured}"

# --- Wildcard pattern ---
match "anything":
    case _:
        matched = True
assert matched is True

# --- OR pattern with literals ---
def vowel(c):
    match c:
        case "a" | "e" | "i" | "o" | "u":
            return True
        case _:
            return False

assert vowel("a") is True
assert vowel("b") is False
assert vowel("u") is True

# --- Sequence patterns ---
match [1, 2, 3]:
    case [a, b, c]:
        seq_result = (a, b, c)
assert seq_result == (1, 2, 3), f"seq_result={seq_result}"

# Sequence with star
match [10, 20, 30, 40]:
    case [first, *rest]:
        star_first = first
        star_rest = rest
assert star_first == 10, f"star_first={star_first}"
assert star_rest == [20, 30, 40], f"star_rest={star_rest}"

# Sequence with star in middle
match [1, 2, 3, 4, 5]:
    case [head, *mid, tail]:
        smid = mid
        stail = tail
assert smid == [2, 3, 4], f"smid={smid}"
assert stail == 5, f"stail={stail}"

# --- Mapping patterns ---
match {"status": 200, "body": "OK"}:
    case {"status": 200, "body": msg}:
        response = msg
assert response == "OK", f"response={response}"

match {"x": 1, "y": 2, "z": 3}:
    case {"x": xv, "y": yv}:
        xy = (xv, yv)
assert xy == (1, 2), f"xy={xy}"

# --- Guard with capture ---
def sign(n):
    match n:
        case x if x > 0:
            return "positive"
        case x if x < 0:
            return "negative"
        case _:
            return "zero"

assert sign(5) == "positive"
assert sign(-3) == "negative"
assert sign(0) == "zero"

# --- Nested sequence pattern ---
match [[1, 2], [3, 4]]:
    case [[p, q], [r, s]]:
        nested = (p, q, r, s)
assert nested == (1, 2, 3, 4), f"nested={nested}"

# --- None and bool literals ---
match None:
    case None:
        none_matched = True
assert none_matched is True

match True:
    case True:
        bool_matched = True
    case _:
        bool_matched = False
assert bool_matched is True

match False:
    case False:
        false_matched = True
assert false_matched is True

# --- No arm matches → nothing bound, no error ---
no_match = "untouched"
match 99:
    case 0:
        no_match = "zero"
assert no_match == "untouched"

# --- match as a regular name still works ---
match = 5
assert match == 5

print("match OK")
