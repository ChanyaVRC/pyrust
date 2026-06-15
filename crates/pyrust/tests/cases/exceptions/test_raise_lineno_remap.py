# Two identical raise statements must each report their own line number;
# the optimizer's remap_linenos must not cross-attribute them (issue #2432).

try:
    raise ValueError("a")
except ValueError as e:
    print(e.__traceback__.tb_lineno)

try:
    raise ValueError("b")
except ValueError as e:
    print(e.__traceback__.tb_lineno)

# Same with TypeError
try:
    raise TypeError("x")
except TypeError as e:
    print(e.__traceback__.tb_lineno)

try:
    raise TypeError("y")
except TypeError as e:
    print(e.__traceback__.tb_lineno)
