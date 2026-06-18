# `abc` module parity (issue #2612).  Exercises the `ABC` / `ABCMeta`
# surface: `@abstractmethod` collection into `__abstractmethods__`, the
# TypeError raised on instantiating an abstract class, inherited-metaclass
# propagation (`type(Shape) is ABCMeta`), concrete-subclass instantiation,
# virtual subclass `.register()`, and `get_cache_token`.
import abc


class Shape(abc.ABC):
    @abc.abstractmethod
    def area(self):
        ...

    @abc.abstractmethod
    def name(self):
        ...


class Circle(Shape):
    def __init__(self, r):
        self.r = r

    def area(self):
        return 3.14 * self.r ** 2

    def name(self):
        return "circle"


class BadShape(Shape):
    pass


# Metaclass propagates to subclasses without an explicit metaclass=.
print(type(Shape) is abc.ABCMeta)
print(type(Circle) is abc.ABCMeta)

# Abstract method names are collected; the concrete subclass clears them.
print(sorted(Shape.__abstractmethods__))
print(sorted(Circle.__abstractmethods__))
print(sorted(BadShape.__abstractmethods__))

# Instantiating an abstract class raises TypeError with CPython's wording.
try:
    BadShape()
except TypeError as e:
    print(e)

# A class still abstract because it only implements one of two methods.
class HalfShape(Shape):
    def area(self):
        return 0


try:
    HalfShape()
except TypeError as e:
    print(e)

# Concrete subclass instantiates fine.
c = Circle(2)
print(round(c.area(), 2), c.name())
print(isinstance(c, Shape))
print(issubclass(Circle, Shape))

# Virtual subclass registration.
token0 = abc.get_cache_token()


class Drawable(abc.ABC):
    @abc.abstractmethod
    def draw(self):
        ...


class Sprite:
    def draw(self):
        return "sprite"


Drawable.register(Sprite)
print(isinstance(Sprite(), Drawable))
print(issubclass(Sprite, Drawable))
print(abc.get_cache_token() > token0)

# get_cache_token returns an int.
print(isinstance(abc.get_cache_token(), int))
