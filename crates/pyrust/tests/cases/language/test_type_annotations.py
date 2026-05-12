# Variable annotations (PEP 526)
x: int = 5
assert x == 5

# Annotation without value (declaration only)
y: str

# Annotation with complex type expression
z: list[int] = [1, 2, 3]
assert z == [1, 2, 3]

# Annotated assignment in a function body
def f():
    a: int = 10
    b: str = "hi"
    return (a, b)

assert f() == (10, "hi")

# Function parameter annotations + return annotation (already supported)
def g(n: int, name: str = "x") -> str:
    return name * n

assert g(3) == "xxx"
assert g(2, name="ab") == "abab"

# Attribute annotation
class C:
    x: int = 0
    def set(self, v: int) -> None:
        self.x = v
        # Method-local annotation
        local: int = v + 1
        return local

c = C()
assert c.x == 0
result = c.set(5)
assert c.x == 5
assert result == 6

# Annotation with simple type names (complex generic syntax with comma needs extended indexing)
flag: bool = True
items: dict = {"a": 1, "b": 2}
assert flag is True
assert items["a"] == 1

# Nested generic types (single-arg form; tuple[int, str] needs extended indexing support)
pair: list[int] = [1, 2]
assert pair == [1, 2]

# Walrus and annotation can coexist
n: int = 0
if (n := 10) == 10:
    assert n == 10

print("type annotations OK")
