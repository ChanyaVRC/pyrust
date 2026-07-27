"""Nested catch_warnings contexts restore the nearest recording sink."""

import warnings


with warnings.catch_warnings(record=True) as outer:
    warnings.simplefilter("always")
    warnings.warn("outer before")
    with warnings.catch_warnings(record=True) as inner:
        warnings.simplefilter("always")
        warnings.warn("inner")
    warnings.warn("outer after")

print("recording nested", [str(item.message) for item in outer])
print("recording inner", [str(item.message) for item in inner])


with warnings.catch_warnings(record=True) as outer:
    warnings.simplefilter("always")
    with warnings.catch_warnings(record=False) as not_recording:
        warnings.simplefilter("always")
        warnings.warn("through non-recording child")

print("non-recording enter", not_recording)
print("recording parent", [str(item.message) for item in outer])


context = warnings.catch_warnings(record=True)
for operation in ("exit before enter", "enter twice"):
    try:
        if operation == "exit before enter":
            context.__exit__(None, None, None)
        else:
            context.__enter__()
            context.__enter__()
    except Exception as exc:
        print(operation, type(exc).__name__, str(exc))
print("first exit is None", context.__exit__(None, None, None) is None)
print("repeated exit is None", context.__exit__(None, None, None) is None)
