# LICM hoists a nested loop's constant `range` argument before the outer loop.
# The inner closed form must preserve final bindings and module insertion order.
tracked_nested_names = {"nested_total", "nested_outer", "nested_inner"}
nested_total = 0
for nested_outer in range(3):
    for nested_inner in range(4):
        nested_total += 1
print("nested", nested_total, nested_outer, nested_inner)
print(
    "nested globals",
    [(name, globals()[name]) for name in globals() if name in tracked_nested_names],
)

# A zero-trip inner range never binds its loop variable, even though the outer
# loop itself runs and leaves its own variable at the final value.
zero_inner_total = 7
for zero_inner_outer in range(2):
    for zero_inner_var in range(0):
        zero_inner_total += 1
print(
    "zero inner",
    zero_inner_total,
    zero_inner_outer,
    "zero_inner_var" in globals(),
)

# The trace only proposes bounds. A shadowed `range` returning a real range
# with different bounds must fail the exact cursor guard on every outer pass
# and resume the original inner loop without losing call order or bindings.
native_range = range
shadow_calls = []


def shadow_range(stop):
    shadow_calls.append(stop)
    return native_range(2)


range = shadow_range
shadow_total = 0
for shadow_outer in native_range(3):
    for shadow_inner in range(5):
        shadow_total += 1
print("shadowed inner", shadow_total, shadow_outer, shadow_inner, shadow_calls)
