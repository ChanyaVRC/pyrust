import sys


# Keep only the exception.  A failed filesystem import is removed from
# sys.modules and its child interpreter is gone before the deferred traceback
# is first materialized.
saved = None
try:
    import _traceback_globals_failed_import
except RuntimeError as caught:
    saved = caught

print(
    "failed module removed:",
    "_traceback_globals_failed_import" not in sys.modules,
)

# Walk to explode(), whose globals must belong to the failed imported module,
# not to this catching module.
tb = saved.__traceback__
while tb.tb_next is not None:
    tb = tb.tb_next
frame_globals = tb.tb_frame.f_globals
raised_function = frame_globals["explode"]

print("owner identity:", frame_globals is raised_function.__globals__)
print("initial:", frame_globals["MARKER"])

# f_globals and function.__globals__ are the same live provider in both
# directions, even though the failed module has no sys.modules owner.
frame_globals["MARKER"] = "via-frame"
print("function sees frame write:", raised_function.__globals__["MARKER"])
raised_function.__globals__["MARKER"] = "via-function"
print("frame sees function write:", frame_globals["MARKER"])


# A module-scope catch frame is also materialized only after its child
# interpreter is gone. Its code filename must remain the imported module's
# source path rather than switching to this importing script.
import _traceback_module_catch_owner as catch_owner

catch_tb = catch_owner.saved.__traceback__
print(
    "module catch filename:",
    catch_tb.tb_frame.f_code.co_filename.endswith(
        "_traceback_module_catch_owner.py"
    ),
)
