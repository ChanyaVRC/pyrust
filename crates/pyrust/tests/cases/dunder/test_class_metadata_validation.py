"""Class metadata uses type's validators and metaclass descriptor precedence."""


def show_error(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__, str(error))
    else:
        print(label, "ok")


class DeleteModuleTarget:
    pass


class DeleteNameTarget:
    pass


class DeleteQualnameTarget:
    pass


class DeleteDocTarget:
    pass


class MetadataControl:
    pass


def delete_module():
    del DeleteModuleTarget.__module__


def delete_name():
    del DeleteNameTarget.__name__


def delete_qualname():
    del DeleteQualnameTarget.__qualname__


def delete_doc():
    del DeleteDocTarget.__doc__


show_error("delete-module", delete_module)
show_error("delete-name", delete_name)
show_error("delete-qualname", delete_qualname)
show_error("delete-doc", delete_doc)
print(
    "metadata-control",
    MetadataControl.__module__,
    MetadataControl.__name__,
    MetadataControl.__qualname__,
    MetadataControl.__doc__,
)


def assign_object_module():
    object.__module__ = "changed"


def assign_int_module():
    int.__module__ = "changed"


def assign_object_attribute():
    object.issue_2937 = "changed"


def delete_object_attribute():
    del object.issue_2937


show_error("object-immutable", assign_object_module)
show_error("int-immutable", assign_int_module)
show_error("object-generic-set", assign_object_attribute)
show_error("object-generic-delete", delete_object_attribute)
print("builtin-modules", object.__module__, int.__module__)


class Text(str):
    pass


class RenameTarget:
    pass


exact_name = "".join(("Exact", "Name"))
RenameTarget.__name__ = exact_name
exact_instance = RenameTarget()
print(
    "exact-name-identity",
    RenameTarget.__name__,
    type(RenameTarget.__name__).__name__,
    RenameTarget.__name__ is exact_name,
    type(exact_instance).__name__ is exact_name,
)

new_name = Text("Renamed")
RenameTarget.__name__ = new_name
renamed_instance = RenameTarget()
print(
    "str-subclass-name",
    RenameTarget.__name__,
    type(RenameTarget.__name__).__name__,
    RenameTarget.__name__ is new_name,
    type(renamed_instance).__name__ is new_name,
)

RenameTarget.__module__ = None
print("renamed-repr", repr(RenameTarget))


class SpoofedText:
    pass


spoofed = SpoofedText()
spoofed.__builtin_data__ = "Spoofed"


def assign_spoofed_name():
    RenameTarget.__name__ = spoofed


show_error("spoofed-name", assign_spoofed_name)
print(
    "name-survives",
    RenameTarget.__name__,
    type(RenameTarget.__name__).__name__,
    RenameTarget.__name__ is new_name,
)

RenameTarget.__qualname__ = "QualifiedTarget"
RenameTarget.__module__ = "plain.module"
print("plain-metadata", RenameTarget.__qualname__, RenameTarget.__module__)


class NonDataModule:
    def __get__(self, instance, owner):
        return "non-data descriptor"


class NonDataMeta(type):
    __module__ = NonDataModule()


class NonDataTarget(metaclass=NonDataMeta):
    pass


print("metaclass-non-data", NonDataTarget.__module__)


class DataMeta(type):
    @property
    def __module__(cls):
        return "data descriptor"


class DataTarget(metaclass=DataMeta):
    pass


print("metaclass-data", DataTarget.__module__)
