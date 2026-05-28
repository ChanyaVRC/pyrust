class CustomNew:
    def __new__(cls, x): return super().__new__(cls)

try:
    CustomNew()
except TypeError as e:
    print(e)

class Foo:
    def __new__(cls, a, b): return super().__new__(cls)

try:
    Foo()
except TypeError as e:
    print(e)

class Bar:
    def __new__(cls, a, b, c): return super().__new__(cls)

try:
    Bar(1)
except TypeError as e:
    print(e)
