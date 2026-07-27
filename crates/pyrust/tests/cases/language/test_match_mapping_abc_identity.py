from collections.abc import Mapping as MappingABC


class UserMapping(MappingABC):
    def __init__(self, values):
        self.values = values

    def __getitem__(self, key):
        return self.values[key]

    def __iter__(self):
        return iter(self.values)

    def __len__(self):
        return len(self.values)


def match_value(value):
    match value:
        case {"answer": answer}:
            return ("matched", answer)
        case _:
            return ("missed", None)


print(match_value(UserMapping({"answer": 42})))
MappingABC.__name__ = "RenamedMapping"
print(match_value(UserMapping({"answer": 43})))


class Mapping:
    def __init__(self):
        self.answer = 44

    def __getitem__(self, key):
        return self.answer

    def keys(self):
        return ("answer",)


print(match_value(Mapping()))
