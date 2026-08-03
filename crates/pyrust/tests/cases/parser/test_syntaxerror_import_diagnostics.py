# A parser failure in an imported source module must be a catchable
# SyntaxError, and the failed module must not remain cached (issue #2855).

import sys

module_name = "syntax_error_import_helper"

try:
    __import__(module_name)
    print("no error")
except SyntaxError as error:
    print(type(error).__name__ + ": " + str(error.args[0]))

print("cached:", module_name in sys.modules)
