# Parity fixture for issue #1677: type() with two solid primitive bases must
# raise TypeError: multiple bases have instance lay-out conflict.

# --- Cases that must raise TypeError ---

try:
    type("D", (int, str), {})
except TypeError as e:
    print(e)

try:
    type("E", (int, float), {})
except TypeError as e:
    print(e)

try:
    type("F", (str, bytes), {})
except TypeError as e:
    print(e)

try:
    type("G", (tuple, list), {})
except TypeError as e:
    print(e)

try:
    type("H", (list, dict), {})
except TypeError as e:
    print(e)

try:
    type("I", (set, frozenset), {})
except TypeError as e:
    print(e)

# class statement syntax
try:
    class Bad(int, str):
        pass
except TypeError as e:
    print(e)

# --- Cases that must succeed ---

type("A", (int,), {})
print("int ok")

type("B", (float,), {})
print("float ok")

type("C", (int, object), {})
print("int+object ok")

type("D2", (str,), {})
print("str ok")

class GoodInt(int):
    pass

print("class GoodInt(int) ok")
