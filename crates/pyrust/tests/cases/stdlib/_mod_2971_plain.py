"""Helper plain module without module-level annotations for issue #2971."""

VAL = 1


def check_bare_annotations():
    try:
        return __annotations__
    except NameError as exc:
        return f"NameError: {exc}"


def fn_with_local_ann(a: int) -> int:
    b: str = "local"
    return a + len(b)
