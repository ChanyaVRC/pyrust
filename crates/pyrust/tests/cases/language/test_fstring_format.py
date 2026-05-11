# f-string uses format() / __format__ semantics, not str()

class Color:
    def __format__(self, spec):
        if spec == "":
            return "Color(default)"
        if spec == "hex":
            return "#ff0000"
        return f"Color({spec})"

c = Color()
assert f"{c}" == "Color(default)", f"got {f'{c}'!r}"
assert f"{c:hex}" == "#ff0000", f"got {f'{c:hex}'!r}"
assert f"{c:rgb}" == "Color(rgb)", f"got {f'{c:rgb}'!r}"

# format() builtin delegates to __format__
assert format(c) == "Color(default)"
assert format(c, "hex") == "#ff0000"

# Built-in types: empty spec behaves like str()
assert f"{42}" == "42"
assert f"{3.14}" == "3.14"
assert f"{True}" == "True"
assert f"{None}" == "None"

print("fstring format OK")
