import sys
import warnings as old_warnings


old_warning_message = old_warnings.WarningMessage
old_catch_warnings = old_warnings.catch_warnings
old_context = old_catch_warnings(record=True)

with old_context as old_log:
    old_warnings.simplefilter("always")
    old_warnings.warn("from old")

    # Re-importing after deleting the cache entry executes warnings.py again,
    # so its Python-defined classes belong to a fresh module generation.
    del sys.modules["warnings"]
    import warnings as new_warnings

    print(
        "reimport:",
        old_warnings is not new_warnings,
        old_warning_message is not new_warnings.WarningMessage,
        old_catch_warnings is not new_warnings.catch_warnings,
    )
    print(
        "old context type:",
        type(old_context) is old_catch_warnings,
        type(old_context) is new_warnings.catch_warnings,
    )

print(
    "old record:",
    len(old_log),
    type(old_log[0]) is old_warning_message,
    type(old_log[0]) is new_warnings.WarningMessage,
)

with new_warnings.catch_warnings(record=True) as new_log:
    new_warnings.simplefilter("always")
    new_warnings.warn("from new")

print(
    "new record:",
    len(new_log),
    type(new_log[0]) is new_warnings.WarningMessage,
    type(new_log[0]) is old_warning_message,
)
print(
    "modules:",
    old_warning_message.__module__,
    new_warnings.WarningMessage.__module__,
    old_catch_warnings.__module__,
    new_warnings.catch_warnings.__module__,
)
