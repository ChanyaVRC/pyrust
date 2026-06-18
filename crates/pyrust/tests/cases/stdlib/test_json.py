import json

# --- dumps: scalars -------------------------------------------------------
print(json.dumps(None))
print(json.dumps(True))
print(json.dumps(False))
print(json.dumps(42))
print(json.dumps(3.14))
print(json.dumps("hello"))

# --- dumps: containers ----------------------------------------------------
print(json.dumps({"a": 1, "b": [2, 3]}))
print(json.dumps([1, "hello", 3.14]))
print(json.dumps((1, 2, 3)))
print(json.dumps({"x": [1, {"y": 2}]}))

# --- dumps: string escapes ------------------------------------------------
print(json.dumps("tab\there\nnewline\"quote\\back"))
print(json.dumps("\x00\x1f"))
print(json.dumps("ünïcodé"))
print(json.dumps("ünïcodé", ensure_ascii=False))
print(json.dumps("emoji \U0001f600"))

# --- dumps: indent / separators / sort_keys -------------------------------
print(json.dumps({"a": 1}, indent=2))
print(json.dumps({"x": [1, 2]}, indent=4))
print(json.dumps([], indent=2))
print(json.dumps({}, indent=2))
print(json.dumps({"b": 2, "a": 1}, sort_keys=True))
print(json.dumps({"a": 1, "b": 2}, separators=(",", ":")))

# --- dumps: non-str keys --------------------------------------------------
print(json.dumps({1: "a", 2.5: "b", None: "d"}))

# --- dumps: default callback ----------------------------------------------
print(json.dumps({1, 2, 3}, default=lambda o: sorted(o)))

# --- loads: scalars -------------------------------------------------------
print(json.loads("null"))
print(json.loads("true"))
print(json.loads("false"))
print(json.loads("42"))
print(json.loads("3.5"))
print(json.loads("-2"))
print(json.loads("1.5e3"))
print(json.loads('"hello"'))

# --- loads: containers ----------------------------------------------------
data = json.loads('{"x": 1, "y": [1, 2, 3]}')
print(data["x"], data["y"])
print(json.loads("[1, 2, 3]"))
print(json.loads('  {"a": "b\\nc", "n": 3.5, "neg": -2}  '))
print(json.loads("[]"))
print(json.loads("{}"))

# --- loads: string escapes ------------------------------------------------
print(json.loads('"\\u00e9\\u0041"'))
print(json.loads('"\\ud83d\\ude00"'))
print(json.loads('"a\\tb\\nc"'))

# --- error handling -------------------------------------------------------
print(issubclass(json.JSONDecodeError, ValueError))
for bad in ["invalid", '{"a": 1,}', "[1, 2", '{bad: 1}', ""]:
    try:
        json.loads(bad)
    except json.JSONDecodeError as e:
        print("JSONDecodeError", e.pos)

try:
    json.dumps({1, 2})
except TypeError as e:
    print("TypeError:", str(e))

try:
    json.dumps(object(), default=None)
except TypeError as e:
    print("TypeError:", str(e))
