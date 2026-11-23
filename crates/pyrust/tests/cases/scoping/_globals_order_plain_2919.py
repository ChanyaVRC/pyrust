"""Plain imported module without module-level annotations (issue #2919)."""

x = 1
y = 2


def helper(v: int) -> int:
    local_ann: str = "ignored"
    return v + len(local_ann)
