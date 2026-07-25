import re


pattern = re.compile(r"\d+")
iterator = pattern.finditer("a1b22c333")
print(iter(iterator) is iterator)

first = next(iterator)
print(first.group(), first.span(), first.pos, first.endpos)
print([(match.group(), match.span()) for match in iterator])

try:
    next(iterator)
except StopIteration:
    print("exhausted")

# Empty matches must advance by one codepoint and include the end position.
print([match.span() for match in re.finditer(r"", "ab")])
print([match.span() for match in re.finditer(r"x*", "abxd")])

# Pattern bounds and metadata are retained by each lazily-created Match.
bounded = list(pattern.finditer("a1b22c333", 3, 6))
print([(match.group(), match.span(), match.pos, match.endpos) for match in bounded])
print(list(pattern.finditer("123", 3, 1)))

# Argument validation remains eager at finditer() call time.
for args in ((123,), ("abc", "bad"), ("abc", 0, "bad")):
    try:
        pattern.finditer(*args)
    except Exception as exc:
        print(type(exc).__name__)
