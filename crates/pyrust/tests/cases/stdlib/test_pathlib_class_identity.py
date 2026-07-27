from pathlib import Path as BuiltinPath


class Path:
    def __init__(self, value):
        self._path = value


real = BuiltinPath("root/same")
fake = Path("root/same")
print("same-name fake:", real == fake)


class RenamedPath(BuiltinPath):
    pass


RenamedPath.__name__ = "NotPathByName"
renamed = RenamedPath("root/same")
print(
    "renamed real:",
    type(renamed) is RenamedPath,
    renamed == real,
    real == renamed,
)
print(
    "derived class:",
    type(renamed / "child") is RenamedPath,
    type(renamed.parent) is RenamedPath,
    type(renamed.with_name("other")) is RenamedPath,
)
print(
    "derived classmethods:",
    type(RenamedPath.cwd()) is RenamedPath,
    type(RenamedPath.home()) is RenamedPath,
    type(renamed.cwd()) is RenamedPath,
    type(renamed.home()) is RenamedPath,
)
