# SyntaxError structured attributes — issue #1087

# 1-arg: msg=args[0], all others None
e = SyntaxError('bad syntax')
print(repr(e.msg))        # 'bad syntax'
print(repr(e.filename))   # None
print(repr(e.lineno))     # None
print(repr(e.offset))     # None
print(repr(e.text))       # None
print(repr(e.end_lineno)) # None
print(repr(e.end_offset)) # None

# 0-arg: all None
e0 = SyntaxError()
print(repr(e0.msg))                # None
print(repr(e0.filename))           # None
print(repr(e0.print_file_and_line))  # None

# 2-arg with 4-element tuple: unpack filename/lineno/offset/text
e2 = SyntaxError('bad', ('file.py', 1, 5, 'x = ;'))
print(repr(e2.msg))       # 'bad'
print(repr(e2.filename))  # 'file.py'
print(repr(e2.lineno))    # 1
print(repr(e2.offset))    # 5
print(repr(e2.text))      # 'x = ;'
print(repr(e2.end_lineno))  # None
print(repr(e2.end_offset))  # None

# 2-arg with 6-element tuple: also sets end_lineno/end_offset
e2b = SyntaxError('bad', ('file.py', 1, 5, 'x = ;', 2, 8))
print(repr(e2b.end_lineno))  # 2
print(repr(e2b.end_offset))  # 8

# OSError structured attributes — issue #1087

# 2-arg: errno + strerror
e4 = OSError(1, 'perm denied')
print(repr(e4.errno))     # 1
print(repr(e4.strerror))  # 'perm denied'
print(repr(e4.filename))  # None
print(repr(e4.filename2)) # None

# 3-arg: errno + strerror + filename
e5 = OSError(1, 'perm denied', 'path.txt')
print(repr(e5.errno))     # 1
print(repr(e5.strerror))  # 'perm denied'
print(repr(e5.filename))  # 'path.txt'
print(repr(e5.filename2)) # None

# 0-arg: all None
e6 = OSError()
print(repr(e6.errno))     # None
print(repr(e6.strerror))  # None
print(repr(e6.filename))  # None
print(repr(e6.filename2)) # None

# 1-arg: all None (not the 2-arg form)
e7 = OSError('simple')
print(repr(e7.errno))     # None
print(repr(e7.strerror))  # None

# Subclasses inherit structured attrs
e8 = FileNotFoundError(2, 'No such file', 'test.txt')
print(repr(e8.errno))     # 2
print(repr(e8.strerror))  # 'No such file'
print(repr(e8.filename))  # 'test.txt'

# IOError alias
e9 = IOError(5, 'io error')
print(repr(e9.errno))     # 5
print(repr(e9.strerror))  # 'io error'

# EnvironmentError alias
e10 = EnvironmentError(13, 'permission denied', '/etc/shadow')
print(repr(e10.errno))     # 13
print(repr(e10.strerror))  # 'permission denied'
print(repr(e10.filename))  # '/etc/shadow'
