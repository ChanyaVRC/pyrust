import __future__ as future_module
import copy
import os
import pathlib
import sys
from collections.abc import Iterable


# Python-defined module classes are generation-local. Objects from an old
# generation retain their original class and behaviour.
old_feature = future_module.annotations
old_feature_class = type(old_feature)
print(
    "future generation:",
    old_feature_class is future_module._Feature,
    type(future_module.nested_scopes) is old_feature_class,
    old_feature_class.__module__,
)
del sys.modules["__future__"]
import __future__ as reloaded_future

print(
    "future reimport:",
    reloaded_future._Feature is old_feature_class,
    type(reloaded_future.annotations) is reloaded_future._Feature,
    type(old_feature) is old_feature_class,
    repr(old_feature) == repr(reloaded_future.annotations),
)

old_copy_error = copy.Error
old_copy_instance = old_copy_error("old")
print(
    "copy generation:",
    copy.error is old_copy_error,
    old_copy_error.__module__,
    isinstance(old_copy_instance, Exception),
)
del sys.modules["copy"]
import copy as reloaded_copy

print(
    "copy reimport:",
    reloaded_copy.Error is old_copy_error,
    reloaded_copy.error is reloaded_copy.Error,
    type(old_copy_instance) is old_copy_error,
    isinstance(old_copy_instance, reloaded_copy.Error),
)

old_environ = os.environ
old_environ_class = type(old_environ)
print("os generation:", old_environ_class.__module__, len(old_environ) >= 0)
del sys.modules["os"]
import os as reloaded_os

print(
    "os reimport:",
    type(reloaded_os.environ) is old_environ_class,
    type(old_environ) is old_environ_class,
    len(old_environ) >= 0,
    len(reloaded_os.environ) >= 0,
)

old_path_class = pathlib.Path
old_posix_class = pathlib.PosixPath
old_path = old_path_class("old/root")
print(
    "pathlib generation:",
    type(old_path) is old_posix_class,
    old_posix_class.__base__ is old_path_class,
    old_path_class.__module__,
    old_posix_class.__module__,
)
del sys.modules["pathlib"]
import pathlib as reloaded_pathlib

new_path = reloaded_pathlib.Path("new/root")
old_path_after_reload = old_path_class("old/again")
print(
    "pathlib reimport:",
    reloaded_pathlib.Path is old_path_class,
    reloaded_pathlib.PosixPath is old_posix_class,
    type(new_path) is reloaded_pathlib.PosixPath,
    type(old_path) is old_posix_class,
    type(old_path_after_reload) is old_posix_class,
    type(old_path / "child") is old_posix_class,
)


# Stable C/frozen ABC identities remain stable across module use, but their
# mutable display name must never select structural semantics.
class IterOnly:
    def __iter__(self):
        return iter(())


class DerivedIterable(Iterable):
    pass


print(
    "abc before rename:",
    issubclass(IterOnly, Iterable),
    isinstance(IterOnly(), Iterable),
    issubclass(IterOnly, DerivedIterable),
)
Iterable.__name__ = "RenamedIterable"
print(
    "abc after rename:",
    issubclass(IterOnly, Iterable),
    isinstance(IterOnly(), Iterable),
    issubclass(IterOnly, DerivedIterable),
    Iterable.__name__,
)
