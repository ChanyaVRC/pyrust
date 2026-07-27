import builtins


# Accessing the function globals for the first time happens in the child
# Interpreter used by source imports. The owning root's active Script mirror
# must still be included in the initial snapshot and observe this mutation.
provider = builtins._namespace_mirror_parent_function.__globals__
seen_before = provider["cross_import_value"]
provider["cross_import_value"] = 72
