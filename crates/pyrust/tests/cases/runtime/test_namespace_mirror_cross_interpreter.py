import builtins


cross_import_value = 71


def parent_function():
    return cross_import_value


builtins._namespace_mirror_parent_function = parent_function
try:
    import _namespace_mirror_import_helper as import_helper

    print(
        "cross interpreter first exposure:",
        import_helper.seen_before,
        cross_import_value,
        parent_function(),
    )
finally:
    del builtins._namespace_mirror_parent_function


# No-argument exec shares the caller's root namespace. The inner Script frame
# is newer and must win first-exposure snapshots; a global write from its
# function must update both the inner and suspended outer fastlocal views.
nested_value = 81
exec(
    "nested_value = 82\n"
    "def nested_write():\n"
    "    global nested_value\n"
    "    nested_value = 83\n"
    "nested_write()\n"
    "nested_snapshot = globals()\n"
    "print('nested mirror inner:', nested_snapshot['nested_value'], nested_value)\n"
)
print("nested mirror outer:", nested_value)
