# CPython 3.12.3 parity tests for the 100% native `importlib.util`
# (issue #3470).  Everything printed is deterministic and identical on every
# host: filesystem locations are shown relative to the temporary directory
# with forward slashes.

import importlib
import importlib.abc
import importlib.machinery
import importlib.util
import os
import sys
import tempfile
import types
import warnings

import _imp
import importlib.util as util

TMP = os.path.realpath(tempfile.mkdtemp())


def norm(path):
    """Show a path relative to TMP with forward slashes."""
    if not isinstance(path, str):
        return repr(path)
    if path.startswith(TMP):
        path = "<tmp>" + path[len(TMP):]
    return path.replace("\\", "/")


def clean(text):
    text = str(text)
    text = text.replace(TMP.replace("\\", "\\\\"), "<tmp>").replace(TMP, "<tmp>")
    return text.replace("\\\\", "/")


def show(label, fn, *args, **kwargs):
    try:
        result = fn(*args, **kwargs)
    except BaseException as exc:
        print(label, "->", type(exc).__name__ + ":", clean(exc))
    else:
        print(label, "->", repr(result))


def write(rel, text):
    path = os.path.join(TMP, *rel.split("/"))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    return path


print("== module surface")
for name in [
    "MAGIC_NUMBER", "_RAW_MAGIC_NUMBER", "cache_from_source", "source_from_cache",
    "decode_source", "resolve_name", "find_spec", "spec_from_loader",
    "spec_from_file_location", "module_from_spec", "source_hash", "LazyLoader",
    "Loader", "_LazyModule", "_resolve_name", "_find_spec", "_find_spec_from_path",
    "_incompatible_extension_module_restrictions",
]:
    print(name, hasattr(util, name))
print(util.__doc__)
print(util.MAGIC_NUMBER, util._RAW_MAGIC_NUMBER)
print(util.Loader is importlib.abc.Loader)
print(importlib.util is util, sys.modules["importlib.util"] is util)

print("== resolve_name")
show("abs", util.resolve_name, "os.path", None)
show("abs2", util.resolve_name, "foo", "pkg")
show("rel1", util.resolve_name, ".mod", "pkg")
show("rel1 bare", util.resolve_name, ".", "pkg")
show("rel2", util.resolve_name, "..mod", "pkg.sub")
show("rel2 bare", util.resolve_name, "..", "pkg.sub")
show("rel3", util.resolve_name, "...a.b", "x.y.z")
show("no package", util.resolve_name, ".mod", None)
show("empty package", util.resolve_name, ".mod", "")
show("beyond top", util.resolve_name, "..mod", "pkg")
show("beyond top 3", util.resolve_name, "...mod", "a.b")
show("not str name", util.resolve_name, 5, "pkg")
show("not str package", util.resolve_name, ".a", 5)
show("one arg", util.resolve_name, "a")
show("_resolve_name", util._resolve_name, "x", "a.b", 2)
show("_resolve_name empty", util._resolve_name, "", "a.b", 1)
show("_resolve_name beyond", util._resolve_name, "x", "a", 2)

print("== decode_source")
cases = [
    b"", b"x = 1", b"a\r\nb\rc\nd", b"a\r", b"a\r\r", b"a\r\n\r", b"\r\n",
    b"\xef\xbb\xbfx = 1\r\n",
    b"# -*- coding: latin-1 -*-\nx = '\xe9'\r\n",
    b"#!/usr/bin/python\n# vim: set fileencoding=latin-1 :\ny = '\xe9'\r",
    b"# coding: utf8\nz = '\xc3\xa9'\r\n",
    b"\n# coding: latin-1\nx = 'ok'",
    b"# coding: UTF-8-SIG\n\xef\xbb\xbfa",
    b"# coding: foo\nx",
    b"\xef\xbb\xbf# coding: latin-1\n",
    b"x = \xff",
    b"# coding: ascii\n\xe9",
    bytearray(b"a\r\nb"),
    "str",
    memoryview(b"ab"),
    None,
]
for source in cases:
    label = repr(source) if not isinstance(source, memoryview) else "memoryview"
    show("decode " + label, util.decode_source, source)

print("== cache_from_source / source_from_cache")
tag = sys.implementation.cache_tag
print(tag)
base = os.path.join(TMP, "pkg", "mod.py")
cached = util.cache_from_source(base)
print(norm(cached))
print(norm(util.source_from_cache(cached)) == norm(base))
print(norm(util.cache_from_source(base, optimization="")))
print(norm(util.cache_from_source(base, optimization=1)))
print(norm(util.cache_from_source(base, optimization=2)))
print(norm(util.cache_from_source(base, optimization="opt")))
print(norm(util.cache_from_source(base, optimization="Z9")))
show("opt bad", util.cache_from_source, base, optimization="a-b")
show("opt space", util.cache_from_source, base, optimization=" ")
for opt in ("", 1, 2, "opt", "x1"):
    pyc = util.cache_from_source(base, optimization=opt)
    print(repr(opt), norm(util.source_from_cache(pyc)) == norm(base), pyc.count("."))
print(norm(util.cache_from_source("mod.py")))
print(norm(util.cache_from_source("dir/mod.py")).replace("<tmp>", ""))
print(norm(util.cache_from_source(".py")))
print(norm(util.cache_from_source("noext")))
print(norm(util.cache_from_source("a.b.c.py")))
show("fspath", lambda: norm(util.cache_from_source(os.fspath(base))))


class P:
    def __fspath__(self):
        return base


print(norm(util.cache_from_source(P())))
print(norm(util.source_from_cache(P.__call__ if False else cached)))
show("bytes path", util.cache_from_source, b"a/b.py")
show("int path", util.cache_from_source, 5)
show("none path", util.cache_from_source, None)
show("kw path", lambda: norm(util.cache_from_source(path=base)))
show("no arg", util.cache_from_source)
show("too many", util.cache_from_source, base, None, 3)
show("bad kw", util.cache_from_source, base, bogus=1)

with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    print(norm(util.cache_from_source(base, True)).count("opt-"))
    print(norm(util.cache_from_source(base, False)).endswith("opt-1.pyc"))
    show("both", util.cache_from_source, base, True, optimization=1)
print(sorted({w.category.__name__ for w in caught}))
print(str(caught[0].message))

sep = os.sep
bad_sources = [
    "mod.pyc",
    "__pycache__" + sep + "mod.pyc",
    "x" + sep + "__pycache__" + sep + "mod.cpython-312.opt-.pyc",
    "x" + sep + "__pycache__" + sep + "mod.cpython-312.foo-1.pyc",
    "x" + sep + "__pycache__" + sep + "mod.cpython-312.opt-a_b.pyc",
    "x" + sep + "__pycache__" + sep + "a.b.cpython-312.opt-1.pyc",
    "x" + sep + "other" + sep + "mod.cpython-312.pyc",
    "x" + sep + "__pycache__" + sep + "mod.cpython-312.opt-1.pyc",
    "x" + sep + "__pycache__" + sep + "mod.cpython-312.pyc",
    "__pycache__" + sep + "mod.cpython-312.pyc",
    sep + "__pycache__" + sep + "mod.cpython-312.pyc",
]
for source in bad_sources:
    show("source_from_cache " + source.replace("\\", "/"), lambda p=source: norm(util.source_from_cache(p)))
show("source_from_cache int", util.source_from_cache, 5)
show("source_from_cache bytes", util.source_from_cache, b"x")

print("-- pycache_prefix and cache_tag")
saved_prefix = sys.pycache_prefix
saved_impl_tag = sys.implementation.cache_tag
try:
    prefix = os.path.join(TMP, "prefix")
    sys.pycache_prefix = prefix
    src = os.path.join(TMP, "deep", "dir", "m.py")
    pyc = util.cache_from_source(src)
    print(pyc.startswith(prefix), pyc.endswith("m." + tag + ".pyc"), "__pycache__" in pyc)
    print(norm(util.source_from_cache(pyc)) == norm(src))
    pyc2 = util.cache_from_source(src, optimization=2)
    print(pyc2.endswith("m." + tag + ".opt-2.pyc"))
    print(norm(util.source_from_cache(pyc2)) == norm(src))
    show("outside prefix", lambda: norm(util.source_from_cache(os.path.join(TMP, "x", "m.cpython-312.pyc"))))
finally:
    sys.pycache_prefix = saved_prefix
print(sys.pycache_prefix is saved_prefix)

print("== source_hash")
for payload in [b"", b"a", b"hello", b"abc", b"12345678", b"123456789", b"x" * 15, b"x" * 16, b"y" * 17, bytes(range(40)), b"\x00" * 33]:
    print(len(payload), util.source_hash(payload).hex())
print(util.source_hash(bytearray(b"hello")).hex(), util.source_hash(memoryview(b"hello")).hex())
for key in (0, 1, 42, 3531, 0x0A0D0DCB, 2**31 - 1, -1, -2**31):
    print(key, _imp.source_hash(key, b"hello").hex(), _imp.source_hash(key, b"abc").hex())
print(_imp.source_hash(util._RAW_MAGIC_NUMBER, b"abc") == util.source_hash(b"abc"))
print(_imp.source_hash(key=7, source=b"abc").hex())
show("source_hash str", util.source_hash, "abc")
show("source_hash none", util.source_hash, None)
show("source_hash noargs", util.source_hash)
show("override", _imp._override_multi_interp_extensions_check, 5)

print("== spec_from_loader / spec_from_file_location")
mod_path = write("plain.py", "VALUE = 41\n")
pkg_init = write("apkg/__init__.py", "NAME = __name__\n")
write("apkg/sub.py", "SUB = 'sub'\n")
other_path = write("data.txt", "text\n")


def describe(spec):
    if spec is None:
        return None
    return (
        spec.name,
        type(spec.loader).__name__ if spec.loader is not None else None,
        norm(spec.origin) if spec.origin is not None else None,
        spec.parent,
        None if spec.submodule_search_locations is None else [norm(p) for p in spec.submodule_search_locations],
        spec.has_location,
        norm(spec.cached) if spec.cached is not None else None,
    )


spec = util.spec_from_file_location("plain", mod_path)
print(describe(spec))
print(isinstance(spec, importlib.machinery.ModuleSpec))
print(describe(util.spec_from_file_location("apkg", pkg_init)))
print(describe(util.spec_from_file_location("apkg.sub", os.path.join(TMP, "apkg", "sub.py"))))
print(describe(util.spec_from_file_location("x", other_path)))
print(describe(util.spec_from_file_location("x", other_path, loader=importlib.machinery.SourceFileLoader("x", other_path))))
print(describe(util.spec_from_file_location("plain", mod_path, submodule_search_locations=[])))
print(describe(util.spec_from_file_location("plain", mod_path, submodule_search_locations=["a", "b"])))
print(describe(util.spec_from_file_location("apkg", pkg_init, submodule_search_locations=None)))
print(describe(util.spec_from_file_location("compiled", os.path.join(TMP, "__pycache__", "compiled." + tag + ".pyc"))))
print(describe(util.spec_from_file_location("ext", os.path.join(TMP, "ext" + importlib.machinery.EXTENSION_SUFFIXES[-1]))))


class P2:
    def __fspath__(self):
        return mod_path


print(describe(util.spec_from_file_location("plain", P2())))
cwd = os.getcwd()
os.chdir(TMP)
try:
    print(describe(util.spec_from_file_location("plain", "plain.py")))
    print(describe(util.spec_from_file_location("plain", "." + os.sep + "plain.py")))
finally:
    os.chdir(cwd)
print(describe(util.spec_from_file_location("nofile")) if False else "skip")
show("bad location", util.spec_from_file_location, "x", 5)
show("no args", util.spec_from_file_location)
show("too many", util.spec_from_file_location, "a", "b", "c")
show("bad kw", util.spec_from_file_location, "a", "b", bogus=1)


class Loader1:
    def get_filename(self, name):
        return os.path.join(TMP, "from_loader.py")

    def is_package(self, name):
        return name.endswith("pkg")


print(describe(util.spec_from_file_location("a", loader=Loader1())))
print(describe(util.spec_from_file_location("apkg", loader=Loader1())))
print(describe(util.spec_from_loader("a", Loader1())))
print(describe(util.spec_from_loader("apkg", Loader1())))
print(describe(util.spec_from_loader("a", Loader1(), is_package=True)))
print(describe(util.spec_from_loader("apkg", Loader1(), is_package=False)))
print(describe(util.spec_from_loader("a", Loader1(), origin="orig")))


class Loader2:
    def is_package(self, name):
        raise ImportError("nope")


class Loader3:
    _ORIGIN = "frozen-ish"

    def is_package(self, name):
        return True


class Loader4:
    pass


print(describe(util.spec_from_loader("a", Loader2())))
print(describe(util.spec_from_loader("a", Loader3())))
print(describe(util.spec_from_loader("a", Loader4())))
print(describe(util.spec_from_loader("a", None)))
print(describe(util.spec_from_loader("a", Loader2(), is_package=True)))
show("spec_from_loader missing", util.spec_from_loader, "a")
show("spec_from_loader positional kwonly", util.spec_from_loader, "a", None, "o")

print("== module_from_spec / exec_module")
spec = util.spec_from_file_location("plain", mod_path)
module = util.module_from_spec(spec)
print(type(module).__name__, module.__name__, module.__package__, module.__spec__ is spec, module.__loader__ is spec.loader)
print(norm(module.__file__), norm(module.__cached__), hasattr(module, "__path__"), hasattr(module, "VALUE"))
spec.loader.exec_module(module)
print(module.VALUE)

spec = util.spec_from_file_location("apkg", pkg_init)
module = util.module_from_spec(spec)
print(module.__name__, repr(module.__package__), [norm(p) for p in module.__path__], norm(module.__file__))
spec.loader.exec_module(module)
print(module.NAME)

sub = util.spec_from_file_location("apkg.sub", os.path.join(TMP, "apkg", "sub.py"))
module = util.module_from_spec(sub)
print(module.__package__, hasattr(module, "__path__"))
sub.loader.exec_module(module)
print(module.SUB)


class WithCreate:
    def __init__(self):
        self.made = None

    def create_module(self, spec):
        self.made = types.ModuleType("custom_" + spec.name)
        self.made.pre = 1
        return self.made

    def exec_module(self, module):
        module.ran = True


loader = WithCreate()
spec = util.spec_from_loader("created", loader)
module = util.module_from_spec(spec)
print(module is loader.made, module.__name__, module.pre, module.__spec__ is spec, module.__package__)
loader.exec_module(module)
print(module.ran)


class DefaultCreate:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        pass


module = util.module_from_spec(util.spec_from_loader("dflt", DefaultCreate()))
print(type(module).__name__, module.__name__, module.__loader__.__class__.__name__)


class OnlyExec:
    def exec_module(self, module):
        pass


show("exec only", util.module_from_spec, util.spec_from_loader("e", OnlyExec()))
print(type(util.module_from_spec(util.spec_from_loader("n", None))).__name__)
nspec = importlib.machinery.ModuleSpec("nsp", None, is_package=True)
nspec.submodule_search_locations.append("/some/where")
nmod = util.module_from_spec(nspec)
print(nmod.__file__, nmod.__path__, type(nmod.__loader__).__name__, nspec.loader is nmod.__loader__)
show("module_from_spec none", util.module_from_spec, None)

print("-- _init_module_attrs")
from _frozen_importlib import _init_module_attrs
mod = types.ModuleType("keep")
mod.__package__ = "kept_pkg"
mod.__loader__ = "kept_loader"
mod.__file__ = "kept_file"
spec = util.spec_from_file_location("other", mod_path)
_init_module_attrs(spec, mod)
print(mod.__name__, mod.__package__, mod.__loader__, mod.__file__, mod.__spec__ is spec)
_init_module_attrs(spec, mod, override=True)
print(mod.__name__, mod.__package__, type(mod.__loader__).__name__, norm(mod.__file__))
print(_init_module_attrs(spec, mod) is mod)


class Slotted:
    __slots__ = ()


print(_init_module_attrs(spec, Slotted()).__class__.__name__)

print("== find_spec")
for name in ["sys", "os", "json", "json.decoder", "collections.abc", "importlib.util", "_thread", "builtins"]:
    found = util.find_spec(name)
    print(name, found is not None and found.name, found is not None and found.parent)
print(util.find_spec("zz_missing_module_xyz"))
show("missing submodule", util.find_spec, "zz_missing_pkg.sub")
show("missing submodule of pkg", lambda: util.find_spec("json.zz_missing"))
print(util.find_spec("json.decoder").name, util.find_spec(".decoder", "json").name, util.find_spec("..util", "importlib.machinery").name)
show("relative no package", util.find_spec, ".x")
show("relative beyond", util.find_spec, "..x", "json")
show("find_spec non-str", util.find_spec, 5)

holder = types.ModuleType("zz_holder")
sys.modules["zz_holder"] = holder
show("spec none", util.find_spec, "zz_holder")
del holder.__spec__
show("spec missing", util.find_spec, "zz_holder")
show("spec missing relative", util.find_spec, ".zz_holder", "")
holder.__spec__ = "sentinel"
show("spec set", util.find_spec, "zz_holder")
sys.modules["zz_none"] = None
print(util.find_spec("zz_none"))
show("_find_spec_from_path", util._find_spec_from_path, "zz_holder")
show("_find_spec_from_path none", util._find_spec_from_path, "zz_none")
show("_find_spec_from_path missing", util._find_spec_from_path, "zz_not_there")
del sys.modules["zz_holder"], sys.modules["zz_none"]
show("__main__", util.find_spec, "__main__")
print(util.find_spec(".", "json").name)


class CustomFinder:
    calls = []

    def find_spec(self, name, path=None, target=None):
        self.calls.append((name, path, target))
        if name == "zz_virtual":
            return importlib.machinery.ModuleSpec(name, None, origin="virtual")
        return None


finder = CustomFinder()
sys.meta_path.insert(0, finder)
try:
    virtual = util.find_spec("zz_virtual")
    print(virtual.name, virtual.origin, virtual.loader)
    print(CustomFinder.calls)
    print(util.find_spec("zz_other_virtual"))
    print(CustomFinder.calls[-1])
    print(util._find_spec("zz_virtual", ["p"], "tgt").origin, CustomFinder.calls[-1])
finally:
    sys.meta_path.remove(finder)
print(util.find_spec("zz_virtual"))


class NoFindSpec:
    pass


sys.meta_path.insert(0, NoFindSpec())
try:
    print(util.find_spec("zz_virtual"))
finally:
    del sys.meta_path[0]

saved_meta_path = sys.meta_path
sys.meta_path = None
try:
    show("meta_path none", util._find_spec, "zz_virtual", None)
finally:
    sys.meta_path = saved_meta_path

saved_tag = sys.implementation.cache_tag
sys.implementation.cache_tag = None
try:
    show("cache_from_source no tag", util.cache_from_source, "a.py")
    show("source_from_cache no tag", util.source_from_cache, "a.pyc")
finally:
    sys.implementation.cache_tag = saved_tag
print(util.cache_from_source("a.py").endswith("a." + saved_tag + ".pyc"))

sys.path.insert(0, TMP)
try:
    found = util.find_spec("plain")
    print(found.name, norm(found.origin), type(found.loader).__name__)
    found = util.find_spec("apkg")
    print(found.name, norm(found.origin), [norm(p) for p in found.submodule_search_locations])
    found = util.find_spec("apkg.sub")
    print(found.name, norm(found.origin), found in [found])
    print("apkg" in sys.modules)
    print(util.find_spec(".sub", "apkg").name)
    print(util.find_spec("apkg.nothing"))
finally:
    sys.path.remove(TMP)
    sys.modules.pop("apkg", None)

print("== LazyLoader / _LazyModule")
write("lazymod.py", "print('lazymod executing')\nVALUE = 7\n\ndef func():\n    return 'f'\n")
spec = util.spec_from_file_location("lazymod", os.path.join(TMP, "lazymod.py"))
print(type(spec.loader).__name__)
spec.loader = util.LazyLoader(spec.loader)
print(type(spec.loader).__name__, type(spec.loader.loader).__name__)
module = util.module_from_spec(spec)
sys.modules["lazymod"] = module
spec.loader.exec_module(module)
print("created")
print(type(module).__name__, type(module) is util._LazyModule)
print(module.__name__)
print(module.__spec__ is spec, type(spec.loader).__name__, spec.loader is module.__loader__)
print("accessing VALUE")
print(module.VALUE)
print(module.func())
print(type(module) is types.ModuleType, "lazymod" in sys.modules and sys.modules["lazymod"] is module)
print(isinstance(module, types.ModuleType))
print(sorted(k for k in vars(module) if not k.startswith("__")))
print(spec.loader_state is not None and sorted(spec.loader_state))
del sys.modules["lazymod"]

print("-- attributes set before load survive the load")
write("lazymod2.py", "A = 1\nB = 2\n")
spec = util.spec_from_file_location("lazymod2", os.path.join(TMP, "lazymod2.py"))
spec.loader = util.LazyLoader(spec.loader)
module = util.module_from_spec(spec)
sys.modules["lazymod2"] = module
spec.loader.exec_module(module)
module.A = "overridden"
module.extra = "added"
print(module.B, module.A, module.extra)
del sys.modules["lazymod2"]

print("-- __delattr__ triggers the load")
write("lazymod3.py", "print('lazymod3 executing')\nX = 1\nY = 2\n")
spec = util.spec_from_file_location("lazymod3", os.path.join(TMP, "lazymod3.py"))
spec.loader = util.LazyLoader(spec.loader)
module = util.module_from_spec(spec)
sys.modules["lazymod3"] = module
spec.loader.exec_module(module)
del module.X
print(hasattr(module, "X"), module.Y)
show("del missing", lambda: delattr(module, "nope"))
del sys.modules["lazymod3"]

print("-- load error surfaces on access")
write("lazybad.py", "raise RuntimeError('boom')\n")
spec = util.spec_from_file_location("lazybad", os.path.join(TMP, "lazybad.py"))
spec.loader = util.LazyLoader(spec.loader)
module = util.module_from_spec(spec)
sys.modules["lazybad"] = module
spec.loader.exec_module(module)
show("lazybad access", lambda: module.anything)
del sys.modules["lazybad"]

print("-- substituted module")
write("lazysub.py", "Z = 1\n")
spec = util.spec_from_file_location("lazysub", os.path.join(TMP, "lazysub.py"))
spec.loader = util.LazyLoader(spec.loader)
module = util.module_from_spec(spec)
sys.modules["lazysub"] = module
spec.loader.exec_module(module)
sys.modules["lazysub"] = types.ModuleType("lazysub")
show("substituted", lambda: module.Z)
del sys.modules["lazysub"]

print("-- LazyLoader API")
show("not a loader", util.LazyLoader, object())
show("not a loader (none)", util.LazyLoader, None)
show("factory not a loader", util.LazyLoader.factory, 5)
print(util.LazyLoader.__mro__[1] is importlib.abc.Loader, issubclass(util.LazyLoader, importlib.abc.Loader))
print(isinstance(util.LazyLoader(importlib.machinery.SourceFileLoader("a", "b.py")), importlib.abc.Loader))
print(util.LazyLoader.__module__, util._LazyModule.__module__, util._LazyModule.__mro__[1] is types.ModuleType)


class EagerLoader:
    def __init__(self, tag):
        self.tag = tag
        self.created = []

    def create_module(self, spec):
        self.created.append(spec)
        return None

    def exec_module(self, module):
        module.tag = self.tag


factory = util.LazyLoader.factory(EagerLoader)
lazy = factory("t1")
print(type(lazy).__name__, lazy.loader.tag)
print(lazy.create_module("spec") is None, lazy.loader.created)
show("factory kwargs", lambda: util.LazyLoader.factory(lambda **kw: EagerLoader(kw["x"]))(x="kw").loader.tag)

print("== _incompatible_extension_module_restrictions")
cm = util._incompatible_extension_module_restrictions(disable_check=True)
print(cm.disable_check, cm.override)
show("enter", cm.__enter__)
print(hasattr(cm, "old"))
show("exit", cm.__exit__, None, None, None)
print(util._incompatible_extension_module_restrictions(disable_check=0).override)
show("no kw", util._incompatible_extension_module_restrictions)
show("positional", util._incompatible_extension_module_restrictions, True)
show("override 0", _imp._override_multi_interp_extensions_check, 0)

print("== existing importlib users still work")
print(importlib.import_module("json").__name__)
print(importlib.import_module(".decoder", "json").__name__)
loader_spec = importlib.util.find_spec("pkgutil")
print(loader_spec.name)
print(sorted(set(importlib.machinery.SOURCE_SUFFIXES)))

print("done")
