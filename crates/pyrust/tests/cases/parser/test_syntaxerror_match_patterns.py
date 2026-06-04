# Compile-time SyntaxError for invalid match/case patterns (issue #2144):
#   * duplicate capture names within a pattern
#   * `_` used as a binding target
#   * duplicate mapping-pattern keys
#   * repeated keyword in a class pattern


def check(src):
    try:
        compile(src, "<test>", "exec")
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)


# Duplicate capture names (#2144).
check("match [1, 2]:\n case [a, a]: pass")
check("match {}:\n case {1: a, 2: a}: pass")
check("match []:\n case [a, *a]: pass")
check("match 1:\n case [x] as x: pass")

# `_` as a target (#2144).
check("match 1:\n case _ as _: pass")
check("match 1:\n case 1 as _: pass")

# Duplicate mapping keys (#2144), value-compared, repr-reported.
check('match {}:\n case {"a": 1, "a": 2}: pass')
check("match {}:\n case {None: 1, None: 2}: pass")
check("match {}:\n case {1: a, 1.0: b}: pass")
check("match {}:\n case {1: a, True: b}: pass")

# Repeated keyword in a class pattern (#2144).
check("match 1:\n case object(x=1, x=2): pass")

# Valid neighbors.
match [1, 2]:
    case [a, b]:
        print(a, b)

match {"k": 9}:
    case {"k": v}:
        print(v)

match {1: 10, 2: 20}:
    case {1: _, 2: _}:
        print("wildcards ok")

match [1, 2, 3]:
    case [first, *rest]:
        print(first, rest)

match 5:
    case x if x > 3:
        print("guard", x)
