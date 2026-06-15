# Parity test for issue #2320: str.format_map unbound-call error messages.
#
# format_map is implemented in Python in CPython, so its unbound-call
# diagnostics use the method_descriptor wording, not the slot-wrapper wording:
#   - no receiver:    "unbound method str.format_map() needs an argument"
#   - wrong receiver: "descriptor 'format_map' for 'str' objects doesn't
#                      apply to a '<type>' object"

# --- no argument at all ---
try:
    str.format_map()
    print("FAIL: should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- wrong receiver type (int) ---
try:
    str.format_map(5, {})
    print("FAIL: should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- wrong receiver type (bytes) ---
try:
    str.format_map(b"x", {})
    print("FAIL: should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- wrong receiver type (None) ---
try:
    str.format_map(None, {})
    print("FAIL: should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- wrong receiver type via getattr-then-call ---
f = str.format_map
try:
    f(3.5, {})
    print("FAIL: should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- happy path: unbound call with a str receiver ---
print(str.format_map("hi {x}", {"x": 1}))

# --- happy path: bound method call ---
print("hello {name}".format_map({"name": "world"}))

# --- happy path: empty template, empty mapping ---
print(repr(str.format_map("", {})))

# --- str subclass receiver still works ---
class S(str):
    pass

print(S("a={a}").format_map({"a": 42}))
