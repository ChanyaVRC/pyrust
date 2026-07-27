"""Loop lowering must preserve the source line of nested control flow."""

import sys


def raise_here():
    1 / 0


def continue_case(flag):
    for _ in [0]:
        if flag:
            raise_here()
            continue
        raise_here()


class ExplodingFlag:
    def __bool__(self):
        1 / 0


def break_case():
    while True:
        if ExplodingFlag():
            break
        pass


def continue_line(flag):
    try:
        continue_case(flag)
    except ZeroDivisionError:
        traceback = sys.exc_info()[2]
        while traceback.tb_frame.f_code.co_name != "continue_case":
            traceback = traceback.tb_next
        return traceback.tb_lineno


def break_line():
    try:
        break_case()
    except ZeroDivisionError:
        traceback = sys.exc_info()[2]
        while traceback.tb_frame.f_code.co_name != "break_case":
            traceback = traceback.tb_next
        return traceback.tb_lineno


lines = [continue_line(True), continue_line(False), break_line()]
print(lines)
assert lines[0] != lines[1]
