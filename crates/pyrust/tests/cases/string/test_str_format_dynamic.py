# str.format() dynamic format specs (PEP 3101 one-level nesting).
# {value:{spec_field}} substitutes a positional or keyword argument
# into the format spec before applying it.

# --- keyword spec fields ---
print("{:{width}}".format("hello", width=10))         # left-aligned, width 10
print("{:>{width}}".format("hi", width=5))            # right-aligned, width 5
print("{:>{width}s}".format("hi", width=5))           # explicit 's' type
print("{name:{width}}".format(name="Alice", width=10)) # named value + named spec

# --- positional spec fields (auto-numbered) ---
print("{:{}}".format("hi", 5))           # auto[0]="hi", spec auto[1]=5
print("{:{}}{}" .format("hi", 5, "end")) # auto chain: value, spec, next field

# --- positional spec fields (manual-numbered) ---
print("{0:{1}}".format("x", 5))          # value=arg[0], spec=arg[1]
print("{1:{0}}".format(5, "x"))          # value=arg[1], spec=arg[0]

# --- mixed: named value, named spec ---
width = 8
print("{:>{width}}".format("test", width=width))

# --- regression: basic cases still work ---
print("{}".format(42))
print("{!r}".format("hi"))
print("{0}".format("hello"))
print("{name}".format(name="world"))

# --- format_map with named dynamic spec ---
print("{x:{width}}".format_map({"x": "hello", "width": 10}))
print("{x:>{width}}".format_map({"x": "hi", "width": 5}))

# --- spec with literal text around the field reference ---
print("{:>{width}s}".format("abc", width=7))   # '>7s' after expansion

# --- integer width in dynamic spec ---
print("{:{width}d}".format(42, width=8))   # right-aligned int

# --- format_map: positional refs in spec raise ValueError ---
try:
    "{x:{0}}".format_map({"x": "hi", "0": 5})
except ValueError as e:
    print(type(e).__name__, "positional")
