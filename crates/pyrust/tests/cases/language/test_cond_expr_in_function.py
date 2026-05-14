# Regression tests for issue #361:
# `A if cond else B` inside a function body, when `cond` is a local
# (parameter or local variable) and the result is consumed (assigned then
# read, or used as a sub-expression), previously raised
# `Runtime error: internal: temp register read before write`.

# --- Repro from the issue --------------------------------------------------
def f_repro(c):
    x = 1 if c else 2
    print(x)
f_repro(1)
f_repro(0)

# --- "What also fails" cases from the issue --------------------------------
def f_print_str(c):
    print("a" if c else "b")
f_print_str(1)
f_print_str(0)

def f_assign_then_concat(c):
    x = "OK" if c else "FAIL"
    return x + "!"
print(f_assign_then_concat(1))
print(f_assign_then_concat(0))

def f_nested(c):
    x = (1 if c else 2) if c else 3
    print(x)
f_nested(1)
f_nested(0)

# --- Local variable (not just a parameter) ---------------------------------
def f_local_cond(flag):
    c = flag
    x = 10 if c else 20
    print(x)
f_local_cond(1)
f_local_cond(0)

# --- Ternary directly in a call argument -----------------------------------
def f_in_call(c):
    return abs(-1 if c else -5)
print(f_in_call(1))
print(f_in_call(0))

# --- Ternary as RHS of binop, both branches reachable ----------------------
def f_in_binop(c):
    y = (3 if c else 7) + 1
    return y
print(f_in_binop(1))
print(f_in_binop(0))

# --- "What works (for contrast)" — regression guard ------------------------
# module-scope ternary
c_mod = 1
x_mod = 1 if c_mod else 2
print(x_mod)

# literal condition
def f_lit():
    x = 1 if True else 2
    print(x)
f_lit()

# direct return
def f_return(c):
    return 1 if c else 2
print(f_return(1))
print(f_return(0))

# if/else statement form
def f_ifelse(c):
    if c:
        x = 1
    else:
        x = 2
    print(x)
f_ifelse(1)
f_ifelse(0)

# if/else statement form with consumer (was also affected by const-fold bug)
def f_ifelse_concat(c):
    if c:
        x = "OK"
    else:
        x = "FAIL"
    return x + "!"
print(f_ifelse_concat(1))
print(f_ifelse_concat(0))
