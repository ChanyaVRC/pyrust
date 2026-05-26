# Parity fixture for issue #1150: `raise MyError` (class as operand) must invoke
# user-defined __init__ with default arguments, not bypass it via
# instantiate_exception(class, vec![]).

# raise <class> with defaulted __init__ args
class DefaultErr(Exception):
    def __init__(self, msg="default message"):
        super().__init__(msg)
        self.msg = msg

try:
    raise DefaultErr
except DefaultErr as exc:
    print(exc.args)
    print(exc.msg)


# raise <class> with no-arg __init__
class NoArgErr(Exception):
    def __init__(self):
        super().__init__("no arg default")
        self.custom = True

try:
    raise NoArgErr
except NoArgErr as exc:
    print(exc.args)
    print(exc.custom)


# raise X from <class>: cause class also calls __init__
class CauseErr(Exception):
    def __init__(self):
        super().__init__("cause default")
        self.flagged = True

try:
    raise ValueError("primary") from CauseErr
except ValueError as exc:
    c = exc.__cause__
    print(c.args)
    print(c.flagged)


# Direct call still works (no regression)
class AppError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message

err = AppError(404, "not found")
print(err.code)
print(err.message)
print(str(err))
print(err.args)


# kwargs forwarded to user __init__
class DetailedError(ValueError):
    def __init__(self, msg, *, detail=None):
        super().__init__(msg)
        self.detail = detail

e = DetailedError("bad input", detail="must be int")
print(e.detail)
print(e.args)
