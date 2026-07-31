# Python-source definitions for the `collections` members that are most
# naturally expressed in Python: `namedtuple`, `OrderedDict`, `ChainMap`,
# `UserDict`, `UserList`, and `UserString`.
#
# This source is `include_str!`'d into `collections.rs` and executed once,
# at first import of `collections`, into a throwaway namespace whose
# resulting classes/functions are copied onto the module (issue #1884).
# Transcribed from CPython 3.12's `collections/__init__.py`, adapted to
# pyrust's capabilities:
#
#   - `namedtuple` follows CPython's `eval`-a-lambda + `type()` recipe, but
#     bakes `defaults` into the generated lambda's parameter defaults
#     (pyrust does not support assigning `__new__.__defaults__` after the
#     fact) and uses `property` getters in place of CPython's C-level
#     `_tuplegetter` descriptor.
#   - `OrderedDict` is a thin `dict` subclass that leans on pyrust dicts
#     already being insertion-ordered; it only re-implements the
#     order-aware operations (`move_to_end`, `popitem(last=)`,
#     `__reversed__`, order-sensitive `__eq__`, `__or__`/`__ror__`/`__ior__`).
#   - `ChainMap` / `UserDict` / `UserList` / `UserString` are plain `object`
#     subclasses with the full public method set spelled out (pyrust's
#     `collections.abc` mixins do not yet supply the concrete mixin methods).

_sys_maxsize = 9223372036854775807

_keywords = frozenset((
    'False', 'None', 'True', 'and', 'as', 'assert', 'async', 'await',
    'break', 'class', 'continue', 'def', 'del', 'elif', 'else', 'except',
    'finally', 'for', 'from', 'global', 'if', 'import', 'in', 'is',
    'lambda', 'nonlocal', 'not', 'or', 'pass', 'raise', 'return', 'try',
    'while', 'with', 'yield',
))


def namedtuple(typename, field_names, *, rename=False, defaults=None, module=None):
    """Returns a new subclass of tuple with named fields."""
    if isinstance(field_names, str):
        field_names = field_names.replace(',', ' ').split()
    field_names = list(map(str, field_names))
    typename = str(typename)

    if rename:
        seen = set()
        for index, name in enumerate(field_names):
            # NB: the condition is bound to a local rather than inlined in
            # the `if` head.  pyrust mis-evaluates a multi-line
            # parenthesised `or`-chain used directly as an `if` condition
            # (issue #1884 self-review), so we materialise it first.
            invalid = (not name.isidentifier()
                       or name in _keywords
                       or name.startswith('_')
                       or name in seen)
            if invalid:
                field_names[index] = f'_{index}'
            seen.add(name)

    for name in [typename] + field_names:
        if type(name) is not str:
            raise TypeError('Type names and field names must be strings')
        if not name.isidentifier():
            raise ValueError('Type names and field names must be valid '
                             f'identifiers: {name!r}')
        if name in _keywords:
            raise ValueError('Type names and field names cannot be a '
                             f'keyword: {name!r}')

    seen = set()
    for name in field_names:
        if name.startswith('_') and not rename:
            raise ValueError('Field names cannot start with an underscore: '
                             f'{name!r}')
        if name in seen:
            raise ValueError(f'Encountered duplicate field name: {name!r}')
        seen.add(name)

    field_defaults = {}
    if defaults is not None:
        defaults = tuple(defaults)
        if len(defaults) > len(field_names):
            raise TypeError('Got more default values than field names')
        field_defaults = dict(reversed(list(zip(reversed(field_names),
                                                reversed(defaults)))))

    field_names = tuple(field_names)
    num_fields = len(field_names)
    arg_list = ', '.join(field_names)
    if num_fields == 1:
        arg_list += ','
    repr_fmt = '(' + ', '.join(f'{name}=%r' for name in field_names) + ')'

    # Build __new__ via eval, making the rightmost defaulted fields optional
    # (matching CPython's `__new__.__defaults__` behaviour without mutating the
    # function object).  Default *values* are passed through the eval namespace
    # and referenced by name in the signature rather than baked in via `!r`:
    # `repr(value)` is not a reliable round-trip (`float('inf')`, `nan`, a set,
    # or any object with a non-eval-able `__repr__` would raise / mis-evaluate),
    # and this also preserves object identity for a shared mutable default.
    eval_ns = {'_tuple_new': tuple.__new__}
    sig_parts = []
    for index, name in enumerate(field_names):
        if name in field_defaults:
            default_key = f'_def_{index}'
            eval_ns[default_key] = field_defaults[name]
            sig_parts.append(f'{name}={default_key}')
        else:
            sig_parts.append(name)
    sig = ', '.join(sig_parts)
    body = ', '.join(field_names)
    if num_fields == 1:
        body += ','
    if num_fields:
        code = f'lambda _cls, {sig}: _tuple_new(_cls, ({body}))'
    else:
        code = 'lambda _cls: _tuple_new(_cls, ())'
    __new__ = eval(code, eval_ns)
    __new__.__name__ = '__new__'
    __new__.__doc__ = f'Create new instance of {typename}({arg_list})'

    def _make(cls, iterable):
        result = tuple.__new__(cls, iterable)
        if len(result) != num_fields:
            raise TypeError(f'Expected {num_fields} arguments, got {len(result)}')
        return result
    _make.__doc__ = (f'Make a new {typename} object from a sequence '
                     'or iterable')

    def _replace(self, **kwds):
        result = self._make(map(kwds.pop, field_names, self))
        if kwds:
            raise ValueError(f'Got unexpected field names: {list(kwds)!r}')
        return result
    _replace.__doc__ = (f'Return a new {typename} object replacing specified '
                        'fields with new values')

    def __repr__(self):
        'Return a nicely formatted representation string'
        # NB: `repr_fmt % tuple(self)` rather than CPython's `% self`.
        # pyrust's `str.__mod__` does not yet unpack a *tuple subclass*
        # operand into positional arguments, so without the explicit
        # `tuple(...)` it would treat `self` as a single argument and
        # recurse back into this `__repr__` (issue #1884 self-review).
        return self.__class__.__name__ + repr_fmt % tuple(self)

    def _asdict(self):
        'Return a new dict which maps field names to their values.'
        return dict(zip(self._fields, self))

    def __getnewargs__(self):
        'Return self as a plain tuple.  Used by copy and pickle.'
        return tuple(self)

    class_namespace = {
        '__doc__': f'{typename}({arg_list})',
        '__slots__': (),
        '_fields': field_names,
        '_field_defaults': field_defaults,
        '__new__': __new__,
        '_make': classmethod(_make),
        '_replace': _replace,
        '__repr__': __repr__,
        '_asdict': _asdict,
        '__getnewargs__': __getnewargs__,
        '__match_args__': field_names,
    }
    for index, name in enumerate(field_names):
        class_namespace[name] = _make_field_getter(index, name)

    result = type(typename, (tuple,), class_namespace)
    result.__module__ = module if module is not None else '__main__'
    return result


def _make_field_getter(index, name):
    # A property whose getter returns the tuple element at `index`.  Stands
    # in for CPython's C-level `_tuplegetter` descriptor.
    def getter(self):
        return self[index]
    getter.__doc__ = f'Alias for field number {index}'
    return property(getter)


class OrderedDict(dict):
    'Dictionary that remembers insertion order'

    # NB: pyrust dicts are already insertion-ordered, so OrderedDict only
    # needs to re-implement the order-aware surface.  Unlike CPython we do
    # *not* maintain a separate linked list; ordinary `self[k] = v` /
    # `del self[k]` operate on the inherited (ordered) dict directly.
    # (pyrust does not expose `dict.__setitem__` / `dict.__delitem__` /
    # `dict.__eq__` as callable unbound methods, so the CPython recipe of
    # delegating to them is unavailable — issue #1884 self-review.)

    def __reversed__(self):
        # Route through the live, size-guarded reversed-view iterator
        # (issue #2448) rather than a dead `list(...)[::-1]` snapshot, so
        # mutating the OrderedDict mid-`reversed(od)` raises RuntimeError
        # ("OrderedDict mutated during iteration") just as CPython does.
        return reversed(self.keys())

    def popitem(self, last=True):
        '''Remove and return a (key, value) pair from the dictionary.

        Pairs are returned in LIFO order if last is true or FIFO order if false.
        '''
        if not self:
            raise KeyError('dictionary is empty')
        if last:
            key = list(self.keys())[-1]
        else:
            key = list(self.keys())[0]
        value = self[key]
        del self[key]
        return key, value

    def move_to_end(self, key, last=True):
        '''Move an existing element to the end (or beginning if last is false).

        Raise KeyError if the element does not exist.
        '''
        if key not in self:
            raise KeyError(key)
        # CPython's `_odict_move_to_end` returns immediately when the node is
        # already the one being moved to, leaving `od_state` untouched -- so a
        # live iterator must not observe a mutation.  pyrust relinks by
        # deleting and reinserting, which would bump the entry-order
        # generation, so the no-op has to be recognised here (issue #2931).
        # The end is read off `self.keys()` -- the same inherited view
        # `popitem` already leans on.  `last=True` uses the reverse cursor,
        # which is O(1) to create; the `last=False` branch below already walks
        # the whole key order anyway.
        if last:
            end = next(reversed(self.keys()))
        else:
            end = next(iter(self.keys()))
        # The node CPython short-circuits on is the one `_odict_find_node`
        # resolves `key` to, i.e. a dict lookup: the hash has to agree before
        # equality is consulted at all.  Comparing `end == key` on its own is
        # not that test -- two keys that compare equal but hash differently
        # occupy two separate entries, and asking a key whose `__eq__` rejects
        # foreign objects would raise where CPython simply relinks.
        # `is` stays first so a key that is not equal to itself (NaN) is still
        # recognised as the node already sitting at that end.
        if end is key or (hash(end) == hash(key) and end == key):
            return
        if last:
            value = self[key]
            del self[key]
            self[key] = value
        else:
            # Drop the moved entry with a lookup rather than filtering the key
            # order by `k != key`: equality is not entry identity, so that
            # filter deleted every key merely equal to `key` and asked keys
            # that reject foreign comparison a question CPython never asks.
            value = self[key]
            del self[key]
            items = list(self.items())
            self.clear()
            self[key] = value
            for k, v in items:
                self[k] = v

    def __eq__(self, other):
        '''od.__eq__(y) <==> od==y.  Comparison to another OD is order-sensitive
        while comparison to a regular mapping is order-insensitive.
        '''
        if isinstance(other, OrderedDict):
            return dict(self) == dict(other) and \
                list(self.keys()) == list(other.keys())
        if isinstance(other, dict):
            return dict(self) == other
        return NotImplemented

    def __ne__(self, other):
        result = self.__eq__(other)
        if result is NotImplemented:
            return result
        return not result

    def __repr__(self):
        'od.__repr__() <==> repr(od)'
        # NB: `repr(dict(...))` rather than CPython's `'%s(%r)' % (...)`.
        # pyrust's `%r` formatting does not recurse into a *value's* custom
        # __repr__ when the operand is a container, so a nested OrderedDict
        # would render as the default object repr (issue #1884 self-review).
        name = self.__class__.__name__
        if not self:
            return name + '()'
        return name + '(' + repr(dict(self.items())) + ')'

    def copy(self):
        'od.copy() -> a shallow copy of od'
        return self.__class__(self)

    @classmethod
    def fromkeys(cls, iterable, value=None):
        '''Create a new ordered dictionary with keys from iterable and values
        set to value.
        '''
        self = cls()
        for key in iterable:
            self[key] = value
        return self

    def __or__(self, other):
        if not isinstance(other, dict):
            return NotImplemented
        new = self.__class__(self)
        # `dict(other)` rather than passing `other` directly: pyrust's
        # `dict.update` mis-handles a *dict-subclass* argument (treats it
        # as a pair-sequence), so we normalise to a plain dict first
        # (issue #1884 self-review).
        new.update(dict(other))
        return new

    def __ror__(self, other):
        if not isinstance(other, dict):
            return NotImplemented
        new = self.__class__(other)
        new.update(dict(self))
        return new

    def __ior__(self, other):
        self.update(dict(other))
        return self


class ChainMap:
    '''A ChainMap groups multiple dicts (or other mappings) together
    to create a single, updateable view.
    '''

    def __init__(self, *maps):
        self.maps = list(maps) or [{}]

    def __missing__(self, key):
        raise KeyError(key)

    def __getitem__(self, key):
        for mapping in self.maps:
            try:
                return mapping[key]
            except KeyError:
                pass
        return self.__missing__(key)

    def get(self, key, default=None):
        return self[key] if key in self else default

    def __len__(self):
        return len(set().union(*self.maps))

    def __iter__(self):
        d = {}
        for mapping in reversed(self.maps):
            d.update(dict.fromkeys(mapping))
        return iter(d)

    def __contains__(self, key):
        return any(key in m for m in self.maps)

    def __bool__(self):
        return any(self.maps)

    def __repr__(self):
        return f'{self.__class__.__name__}({", ".join(map(repr, self.maps))})'

    @classmethod
    def fromkeys(cls, iterable, *args):
        'Create a ChainMap with a single dict created from the iterable.'
        return cls(dict.fromkeys(iterable, *args))

    def copy(self):
        'New ChainMap or subclass with a new copy of maps[0] and refs to maps[1:]'
        return self.__class__(self.maps[0].copy(), *self.maps[1:])

    __copy__ = copy

    def new_child(self, m=None, **kwargs):
        '''New ChainMap with a new map followed by all previous maps.'''
        if m is None:
            m = kwargs
        elif kwargs:
            m.update(kwargs)
        return self.__class__(m, *self.maps)

    @property
    def parents(self):
        'New ChainMap from maps[1:].'
        return self.__class__(*self.maps[1:])

    def __setitem__(self, key, value):
        self.maps[0][key] = value

    def __delitem__(self, key):
        try:
            del self.maps[0][key]
        except KeyError:
            raise KeyError(f'Key not found in the first mapping: {key!r}')

    def popitem(self):
        'Remove and return an item pair from maps[0]. Raise KeyError is maps[0] is empty.'
        try:
            return self.maps[0].popitem()
        except KeyError:
            raise KeyError('No keys found in the first mapping.')

    def pop(self, key, *args):
        'Remove *key* from maps[0] and return its value.'
        try:
            return self.maps[0].pop(key, *args)
        except KeyError:
            raise KeyError(f'Key not found in the first mapping: {key!r}')

    def clear(self):
        'Clear maps[0], leaving maps[1:] intact.'
        self.maps[0].clear()

    def keys(self):
        return list(self)

    def values(self):
        return [self[k] for k in self]

    def items(self):
        return [(k, self[k]) for k in self]

    def setdefault(self, key, default=None):
        if key not in self:
            self[key] = default
        return self[key]

    def update(self, *args, **kwargs):
        self.maps[0].update(*args, **kwargs)

    def __eq__(self, other):
        if isinstance(other, ChainMap):
            return self.maps == other.maps
        return NotImplemented

    def __ne__(self, other):
        result = self.__eq__(other)
        if result is NotImplemented:
            return result
        return not result

    def __ior__(self, other):
        self.maps[0].update(other)
        return self

    def __or__(self, other):
        if not isinstance(other, dict):
            return NotImplemented
        m = self.copy()
        m.maps[0].update(other)
        return m

    def __ror__(self, other):
        if not isinstance(other, dict):
            return NotImplemented
        m = dict(other)
        for child in reversed(self.maps):
            m.update(child)
        return self.__class__(m)


class UserDict:
    """A more or less complete user-defined wrapper around dictionary objects."""

    def __init__(self, dict=None, /, **kwargs):
        self.data = {}
        if dict is not None:
            self.update(dict)
        if kwargs:
            self.update(kwargs)

    def __len__(self):
        return len(self.data)

    def __getitem__(self, key):
        if key in self.data:
            return self.data[key]
        if hasattr(self.__class__, "__missing__"):
            return self.__class__.__missing__(self, key)
        raise KeyError(key)

    def __setitem__(self, key, item):
        self.data[key] = item

    def __delitem__(self, key):
        del self.data[key]

    def __iter__(self):
        return iter(self.data)

    def __contains__(self, key):
        return key in self.data

    def keys(self):
        return self.data.keys()

    def values(self):
        return self.data.values()

    def items(self):
        return self.data.items()

    def get(self, key, default=None):
        if key in self:
            return self[key]
        return default

    def pop(self, key, *args):
        return self.data.pop(key, *args)

    def popitem(self):
        return self.data.popitem()

    def setdefault(self, key, default=None):
        if key not in self:
            self[key] = default
        return self[key]

    def clear(self):
        self.data.clear()

    def update(self, *args, **kwargs):
        if len(args) > 1:
            raise TypeError(f'update expected at most 1 argument, got {len(args)}')
        if args:
            other = args[0]
            if hasattr(other, 'keys'):
                for k in other.keys():
                    self[k] = other[k]
            else:
                for k, v in other:
                    self[k] = v
        for k in kwargs:
            self[k] = kwargs[k]

    def __repr__(self):
        return repr(self.data)

    def __or__(self, other):
        if isinstance(other, UserDict):
            return self.__class__(self.data | other.data)
        if isinstance(other, dict):
            return self.__class__(self.data | other)
        return NotImplemented

    def __ror__(self, other):
        if isinstance(other, UserDict):
            return self.__class__(other.data | self.data)
        if isinstance(other, dict):
            return self.__class__(other | self.data)
        return NotImplemented

    def __ior__(self, other):
        if isinstance(other, UserDict):
            self.data |= other.data
        else:
            self.data |= other
        return self

    def copy(self):
        if self.__class__ is UserDict:
            return UserDict(self.data.copy())
        c = self.__class__()
        c.data = self.data.copy()
        return c

    @classmethod
    def fromkeys(cls, iterable, value=None):
        d = cls()
        for key in iterable:
            d[key] = value
        return d


class UserList:
    """A more or less complete user-defined wrapper around list objects."""

    def __init__(self, initlist=None):
        self.data = []
        if initlist is not None:
            if type(initlist) == type(self.data):
                self.data[:] = initlist
            elif isinstance(initlist, UserList):
                self.data[:] = initlist.data[:]
            else:
                self.data = list(initlist)

    def __repr__(self):
        return repr(self.data)

    def __lt__(self, other):
        return self.data < self.__cast(other)

    def __le__(self, other):
        return self.data <= self.__cast(other)

    def __eq__(self, other):
        return self.data == self.__cast(other)

    def __gt__(self, other):
        return self.data > self.__cast(other)

    def __ge__(self, other):
        return self.data >= self.__cast(other)

    def __cast(self, other):
        return other.data if isinstance(other, UserList) else other

    def __contains__(self, item):
        return item in self.data

    def __len__(self):
        return len(self.data)

    def __getitem__(self, i):
        if isinstance(i, slice):
            return self.__class__(self.data[i])
        else:
            return self.data[i]

    def __setitem__(self, i, item):
        self.data[i] = item

    def __delitem__(self, i):
        del self.data[i]

    def __add__(self, other):
        if isinstance(other, UserList):
            return self.__class__(self.data + other.data)
        elif isinstance(other, type(self.data)):
            return self.__class__(self.data + other)
        return self.__class__(self.data + list(other))

    def __radd__(self, other):
        if isinstance(other, UserList):
            return self.__class__(other.data + self.data)
        elif isinstance(other, type(self.data)):
            return self.__class__(other + self.data)
        return self.__class__(list(other) + self.data)

    def __iadd__(self, other):
        if isinstance(other, UserList):
            self.data += other.data
        elif isinstance(other, type(self.data)):
            self.data += other
        else:
            self.data += list(other)
        return self

    def __mul__(self, n):
        return self.__class__(self.data * n)

    __rmul__ = __mul__

    def __imul__(self, n):
        self.data *= n
        return self

    def append(self, item):
        self.data.append(item)

    def insert(self, i, item):
        self.data.insert(i, item)

    def pop(self, i=-1):
        return self.data.pop(i)

    def remove(self, item):
        self.data.remove(item)

    def clear(self):
        self.data.clear()

    def copy(self):
        return self.__class__(self)

    def count(self, item):
        return self.data.count(item)

    def index(self, item, *args):
        return self.data.index(item, *args)

    def reverse(self):
        self.data.reverse()

    def sort(self, /, *args, **kwds):
        self.data.sort(*args, **kwds)

    def extend(self, other):
        if isinstance(other, UserList):
            self.data.extend(other.data)
        else:
            self.data.extend(other)


class UserString:
    """A more or less complete user-defined wrapper around string objects."""

    def __init__(self, seq):
        if isinstance(seq, str):
            self.data = seq
        elif isinstance(seq, UserString):
            self.data = seq.data[:]
        else:
            self.data = str(seq)

    def __str__(self):
        return str(self.data)

    def __repr__(self):
        return repr(self.data)

    def __int__(self):
        return int(self.data)

    def __float__(self):
        return float(self.data)

    def __hash__(self):
        return hash(self.data)

    def __getnewargs__(self):
        return (self.data[:],)

    def __eq__(self, string):
        if isinstance(string, UserString):
            return self.data == string.data
        return self.data == string

    def __lt__(self, string):
        if isinstance(string, UserString):
            return self.data < string.data
        return self.data < string

    def __le__(self, string):
        if isinstance(string, UserString):
            return self.data <= string.data
        return self.data <= string

    def __gt__(self, string):
        if isinstance(string, UserString):
            return self.data > string.data
        return self.data > string

    def __ge__(self, string):
        if isinstance(string, UserString):
            return self.data >= string.data
        return self.data >= string

    def __contains__(self, char):
        if isinstance(char, UserString):
            char = char.data
        return char in self.data

    def __len__(self):
        return len(self.data)

    def __getitem__(self, index):
        return self.__class__(self.data[index])

    def __add__(self, other):
        if isinstance(other, UserString):
            return self.__class__(self.data + other.data)
        elif isinstance(other, str):
            return self.__class__(self.data + other)
        return self.__class__(self.data + str(other))

    def __radd__(self, other):
        if isinstance(other, str):
            return self.__class__(other + self.data)
        return self.__class__(str(other) + self.data)

    def __mul__(self, n):
        return self.__class__(self.data * n)

    __rmul__ = __mul__

    def __mod__(self, args):
        return self.__class__(self.data % args)

    def __rmod__(self, template):
        return self.__class__(str(template) % self)

    def capitalize(self):
        return self.__class__(self.data.capitalize())

    def casefold(self):
        return self.__class__(self.data.casefold())

    def center(self, width, *args):
        return self.__class__(self.data.center(width, *args))

    def count(self, sub, start=0, end=_sys_maxsize):
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.count(sub, start, end)

    def removeprefix(self, prefix, /):
        if isinstance(prefix, UserString):
            prefix = prefix.data
        return self.__class__(self.data.removeprefix(prefix))

    def removesuffix(self, suffix, /):
        if isinstance(suffix, UserString):
            suffix = suffix.data
        return self.__class__(self.data.removesuffix(suffix))

    def encode(self, encoding='utf-8', errors='strict'):
        encoding = 'utf-8' if encoding is None else encoding
        errors = 'strict' if errors is None else errors
        return self.data.encode(encoding, errors)

    def endswith(self, suffix, start=0, end=_sys_maxsize):
        return self.data.endswith(suffix, start, end)

    def expandtabs(self, tabsize=8):
        return self.__class__(self.data.expandtabs(tabsize))

    def find(self, sub, start=0, end=_sys_maxsize):
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.find(sub, start, end)

    def format(self, /, *args, **kwds):
        return self.data.format(*args, **kwds)

    def format_map(self, mapping):
        return self.data.format_map(mapping)

    def index(self, sub, start=0, end=_sys_maxsize):
        return self.data.index(sub, start, end)

    def isalpha(self):
        return self.data.isalpha()

    def isalnum(self):
        return self.data.isalnum()

    def isascii(self):
        return self.data.isascii()

    def isdecimal(self):
        return self.data.isdecimal()

    def isdigit(self):
        return self.data.isdigit()

    def isidentifier(self):
        return self.data.isidentifier()

    def islower(self):
        return self.data.islower()

    def isnumeric(self):
        return self.data.isnumeric()

    def isprintable(self):
        return self.data.isprintable()

    def isspace(self):
        return self.data.isspace()

    def istitle(self):
        return self.data.istitle()

    def isupper(self):
        return self.data.isupper()

    def join(self, seq):
        return self.data.join(seq)

    def ljust(self, width, *args):
        return self.__class__(self.data.ljust(width, *args))

    def lower(self):
        return self.__class__(self.data.lower())

    def lstrip(self, chars=None):
        return self.__class__(self.data.lstrip(chars))

    def partition(self, sep):
        return self.data.partition(sep)

    def replace(self, old, new, maxsplit=-1):
        if isinstance(old, UserString):
            old = old.data
        if isinstance(new, UserString):
            new = new.data
        return self.__class__(self.data.replace(old, new, maxsplit))

    def rfind(self, sub, start=0, end=_sys_maxsize):
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.rfind(sub, start, end)

    def rindex(self, sub, start=0, end=_sys_maxsize):
        return self.data.rindex(sub, start, end)

    def rjust(self, width, *args):
        return self.__class__(self.data.rjust(width, *args))

    def rpartition(self, sep):
        return self.data.rpartition(sep)

    def rstrip(self, chars=None):
        return self.__class__(self.data.rstrip(chars))

    def split(self, sep=None, maxsplit=-1):
        return self.data.split(sep, maxsplit)

    def rsplit(self, sep=None, maxsplit=-1):
        return self.data.rsplit(sep, maxsplit)

    def splitlines(self, keepends=False):
        return self.data.splitlines(keepends)

    def startswith(self, prefix, start=0, end=_sys_maxsize):
        return self.data.startswith(prefix, start, end)

    def strip(self, chars=None):
        return self.__class__(self.data.strip(chars))

    def swapcase(self):
        return self.__class__(self.data.swapcase())

    def title(self):
        return self.__class__(self.data.title())

    def translate(self, *args):
        return self.__class__(self.data.translate(*args))

    def upper(self):
        return self.__class__(self.data.upper())

    def zfill(self, width):
        return self.__class__(self.data.zfill(width))
