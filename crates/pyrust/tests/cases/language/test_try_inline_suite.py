# Inline-suite form: statement on the same line as the colon.
# CPython 3.12 accepts `try: <stmt>` as equivalent to `try:\n    <stmt>`.

# Basic try/except inline
try: x = 1
except: pass
print(x)

# try/except inline with exception type
try: raise ValueError()
except ValueError: x = 2
print(x)

# try/except/else inline
results = []
try: y = 10
except: results.append('except')
else: results.append('else')
print(y, results)

# try/except/finally inline
log = []
def record(s): log.append(s)
try: z = 3
except: record('exc')
finally: record('fin')
print(z, log)

# try/except/else/finally all inline
log2 = []
try: a = 42
except: log2.append('except')
else: log2.append('else')
finally: log2.append('finally')
print(a, log2)

# try/finally inline (no except)
log3 = []
try: b = 99
finally: log3.append('done')
print(b, log3)

# except as clause inline
try: raise ValueError('msg')
except ValueError as e: captured = str(e)
print(captured)

# Blank line between inline try-body and continuation keyword
try: ok = True

except: ok = False
print(ok)

# Nested: inline try inside an indented block
def f():
    try: return 1
    except: return 0

print(f())

# Multiline form still works alongside inline
try:
    n = 7
except:
    n = 0
print(n)

# if/elif/else inline (related: same fix covers these too)
v = 0
if False: v = 1
elif True: v = 2
else: v = 3
print(v)

# for/else inline
items = []
for i in range(2): items.append(i)
else: items.append('done')
print(items)

# while/else inline
w = 0
while w < 1: w = 1
else: w = 99
print(w)
