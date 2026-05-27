# Parity fixture for issue #1249: str.format() nested format specs with
# named and auto-numbered fields.
#
# Two gaps existed after the #1079 fix:
# 1. Named precision without type char was silently ignored
#    ("{:.{prec}}".format(3.14, prec=2) returned the full float instead of '3.1').
# 2. Auto-numbered nested spec confirmed to work (no regression).

# --- named precision, no explicit type char (uses g-style significant figures) ---
print("{:.{prec}}".format(3.14159, prec=2))        # 3.1
print("{:.{prec}}".format(3.14159, prec=3))        # 3.14
print("{:.{prec}}".format(3.14159, prec=4))        # 3.142
print("{:.{prec}}".format(3.14159, prec=6))        # 3.14159

# --- named width+precision, no type char on float ---
print("{:{width}.{prec}}".format(3.14, width=10, prec=2))   #        3.1
print("{:{width}.{prec}}".format(3.14159, width=8, prec=3)) #     3.14

# --- named precision with explicit 'f' type char (regression guard from #1079) ---
print("{:.{prec}f}".format(3.14159, prec=2))       # 3.14
print("{:.{prec}f}".format(3.14159, prec=4))       # 3.1416
print("{:.{prec}f}".format(3.14159, prec=0))       # 3

# --- auto-numbered nested spec in width position ---
print("{:{}}".format("hello", 10))                  # 'hello     '
print("{:{}}".format("hi", 5))                      # 'hi   '
print("{:{}}".format(42, 8))                        # '      42'

# --- auto-numbered nested spec in precision position ---
print("{:.{}}".format(3.14159, 2))                  # 3.1
print("{:.{}}".format(3.14159, 3))                  # 3.14
print("{:.{}}".format(3.14159, 4))                  # 3.142

# --- auto-numbered nested spec with f type ---
print("{:.{}f}".format(3.14159, 2))                 # 3.14
print("{:.{}f}".format(3.14159, 4))                 # 3.1416

# --- manually-numbered nested spec ---
print("{0:{1}}".format("hello", 10))                # 'hello     '
print("{1:{0}}".format(10, "hello"))                # 'hello     '

# --- named value with named spec width ---
print("{:{width}}".format("hello", width=10))       # 'hello     '
print("{:{width}}".format("hi", width=5))           # 'hi   '

# --- string with auto-numbered precision ---
print("{:.{}}".format("hello world", 5))            # hello
print("{:.{prec}}".format("hello world", prec=3))   # hel

# --- regression: named precision still works with g ---
print("{:.{prec}g}".format(3.14159, prec=3))        # 3.14
print("{:.{prec}g}".format(3.14159, prec=5))        # 3.1416

# --- prec=0 no type char: CPython uses exponential notation ---
print("{:.{}}".format(3.14159, 0))                  # 3e+00
print("{:.{prec}}".format(3.14159, prec=0))         # 3e+00
print("{:.{}}".format(0.0, 0))                      # 0e+00
print("{:.{}}".format(0.0, 1))                      # 0e+00
print("{:.{}}".format(0.0, 2))                      # 0.0

# --- prec=1 no type char: exp >= 0 triggers exponential ---
print("{:.{}}".format(3.14159, 1))                  # 3e+00
print("{:.{}}".format(10.0, 1))                     # 1e+01
print("{:.{}}".format(0.1, 1))                      # 0.1

# --- fixed notation ensures trailing .0 for integer-valued floats ---
print("{:.{}}".format(10.0, 3))                     # 10.0
print("{:.{}}".format(100.0, 4))                    # 100.0
print("{:.{}}".format(1.0, 2))                      # 1.0
