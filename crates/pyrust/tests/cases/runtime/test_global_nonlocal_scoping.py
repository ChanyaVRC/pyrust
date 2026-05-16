# Exercises nonlocal scoping across two call depths.
# The second and subsequent calls force pool-recycled Environment instances
# to be reused.  A recycled Environment whose local_names / global_names /
# nonlocal_names are not cleared would corrupt name-resolution on those
# subsequent invocations.

# --- Two-level nonlocal, exercised twice (second call recycles envs) ---

def outer():
    x = 0
    def middle():
        nonlocal x
        x += 10
        def inner():
            nonlocal x
            x += 1
        inner()
        return x
    return middle()

print(outer())  # 11
print(outer())  # 11 — recycled envs must still give the same result


# --- Local variables in a reused env must not be treated as nonlocal ---
# If alloc_env leaves stale nonlocal_names containing "value" from a prior
# owner, the second call would route the assignment to an enclosing scope
# instead of the local register, producing the wrong answer.

def use_local():
    value = 42
    return value

print(use_local())  # 42
print(use_local())  # 42 — stale nonlocal_names must not redirect assignment


# --- local_names cleared so lookup_name_in_env does not raise NameError ---
# If local_names is left stale from a prior function that had a local
# named "z", the second call to no_local_z would see "z" as a local name
# (triggering the "not associated with a value" guard) when it is actually
# an enclosing or global name.

z = "module"

def no_local_z():
    return z  # reads from enclosing/global scope, not a local

print(no_local_z())  # module
print(no_local_z())  # module
