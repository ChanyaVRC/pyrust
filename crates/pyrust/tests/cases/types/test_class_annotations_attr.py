class NoAnn:
    x = 1

print(hasattr(NoAnn, '__annotations__'))
print(NoAnn.__annotations__)
print(type(NoAnn.__annotations__).__name__)

# A second class with no annotations also returns empty dict
class Bar:
    pass

print(hasattr(Bar, '__annotations__'))
print(Bar.__annotations__)
