print(
    "__class_getitem__" in dir([]),
    "__class_getitem__" in dir(()),
    "__class_getitem__" in dir({}),
    "__class_getitem__" in dir(set()),
    "__class_getitem__" in dir(frozenset()),
)
print(
    "fromkeys" in dir({}),
    "from_bytes" in dir(1),
    "maketrans" in dir(""),
    "maketrans" in dir(b""),
)
