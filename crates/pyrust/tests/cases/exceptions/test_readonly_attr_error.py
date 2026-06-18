# Assigning (or deleting) a read-only built-in attribute must report the
# right error, matching CPython 3.12 (issue #2562).
#
# CPython distinguishes three cases on a primitive receiver:
#   - read-only getset_descriptor data attr (real/imag/numerator/denominator):
#       AttributeError: attribute 'X' of 'T' objects is not writable
#   - read-only method/method-wrapper (bit_length, upper, append, __add__, ...):
#       AttributeError: 'T' object attribute 'X' is read-only
#   - genuinely absent attribute:
#       AttributeError: 'T' object has no attribute 'X'
# The same wording is used for `del obj.attr`.


def show(code):
    try:
        exec(code, {})
    except Exception as e:
        print(type(e).__name__ + ": " + str(e))


# --- getset_descriptor data attrs -> "is not writable" ---
show("x = 1\nx.real = 5")
show("x = 1\nx.imag = 5")
show("x = 1\nx.numerator = 5")
show("x = 1\nx.denominator = 5")
show("x = 1.0\nx.real = 5")
show("x = 1.0\nx.imag = 5")

# --- read-only methods -> "is read-only" ---
show("x = 1\nx.bit_length = 5")
show("x = 1\nx.conjugate = 5")
show("x = 1.0\nx.conjugate = 5")
show('x = "a"\nx.upper = 5')
show("x = [1]\nx.append = 5")
show("x = ()\nx.count = 5")
show("x = b'y'\nx.hex = 5")

# --- genuinely absent -> "has no attribute" ---
show("x = 1\nx.no_such_attr = 5")
show('x = "a"\nx.totally_missing = 5')

# --- delete path uses the same wording ---
show("x = 1\ndel x.real")
show("x = 1\ndel x.bit_length")
show("x = 1\ndel x.no_such_attr")
