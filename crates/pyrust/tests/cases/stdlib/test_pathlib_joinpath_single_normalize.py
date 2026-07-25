from pathlib import Path, PosixPath


cases = [
    (Path("/"), ("usr", "local", "bin")),
    (Path("//"), ("server", "share", "file")),
    (Path("base"), ("a//b", ".", "c/")),
    (Path("base"), ("left", "/absolute", "tail")),
    (Path("base"), ("left", "///absolute", "tail")),
    (Path("."), ("a", ".", "b")),
    (Path("base"), ("a", "..", "b")),
    (Path("base"), ("", "a", "", "b")),
    (Path("base"), (PosixPath("child"), Path("leaf"))),
]

for base, parts in cases:
    result = base.joinpath(*parts)
    print(str(result), type(result).__name__)

print(str(Path("base").joinpath()))
print(str(Path("base").joinpath("/one", "two", "/three", "four")))

try:
    Path("base").joinpath("ok", 123, "unreached")
except Exception as exc:
    print(type(exc).__name__)
