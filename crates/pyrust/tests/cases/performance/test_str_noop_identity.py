s = "".join(["abcdefghij"] * 100)
noops = [
    s.strip(),
    s.lstrip(),
    s.rstrip(),
    s.replace("missing", "value"),
    s.replace("a", "z", 0),
    s.replace("a", "a"),
    s.removeprefix("missing"),
    s.removeprefix(""),
    s.removesuffix("missing"),
    s.removesuffix(""),
    s.center(1),
    s.ljust(1),
    s.rjust(1),
    s.zfill(1),
    s.expandtabs(),
]

print("str-noop-identity", [value is s for value in noops])
print(
    "str-results",
    s.removeprefix("abc") == s[3:],
    s.removesuffix("hij") == s[:-3],
    "\tab\t".strip().expandtabs(4),
    "aba".replace("a", "xy"),
)
