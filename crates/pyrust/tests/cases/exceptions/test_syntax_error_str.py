# Parity fixture for SyntaxError.__str__ formatting (issue #1213).
# CPython's SyntaxError has a custom tp_str that formats location info as
# "msg (filename, line N)" or "msg (line N)" depending on whether filename
# and lineno structured attrs are set.  SubclassesIndentationError and
# TabError inherit the same formatting.

# 2-arg form: both filename (str) and lineno (int) present
e = SyntaxError("bad syntax", ("myfile.py", 10, 3, "bad code"))
print(str(e))

# 2-arg form: filename is None, lineno present
e2 = SyntaxError("oops", (None, 5, 1, "code"))
print(str(e2))

# IndentationError inherits the same formatting
e3 = IndentationError("unexpected indent", ("test.py", 3, 1, "  x"))
print(str(e3))

# TabError inherits too
e4 = TabError("mixed tabs", ("mixed.py", 7, 2, "\t x"))
print(str(e4))

# 1-arg form: no location → just the message
print(str(SyntaxError("msg")))

# 0-arg form: msg is None → "None"
print(str(SyntaxError()))

# Mutating structured attrs after construction affects __str__
e5 = SyntaxError("bad syntax", ("myfile.py", 10, 3, "bad code"))
e5.filename = "changed.py"
e5.lineno = 99
print(str(e5))

# Non-string filename → treated as no filename (only lineno shown)
print(str(SyntaxError("msg", (42, 10, 3, "code"))))
print(str(SyntaxError("msg", (None, 10, 3, "code"))))

# Non-int lineno → treated as no lineno (only filename shown, if str)
print(str(SyntaxError("msg", ("file.py", None, 3, "code"))))
print(str(SyntaxError("msg", ("file.py", "notint", 3, "code"))))

# Neither filename (str) nor lineno (int) → just msg
print(str(SyntaxError("msg", (None, None, 3, "code"))))

# Non-string msg is still stringified via str()
print(str(SyntaxError(42, ("file.py", 10, 3, "code"))))

# repr is not affected — still uses full args
print(repr(SyntaxError("bad syntax", ("myfile.py", 10, 3, "bad code"))))
