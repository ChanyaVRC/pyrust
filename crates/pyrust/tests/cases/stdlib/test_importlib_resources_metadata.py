import abc
import importlib
import importlib.abc
import importlib.machinery
import importlib.metadata
import importlib.readers
import importlib.resources
import importlib.resources.abc
import importlib.resources.readers
import importlib.resources.simple
import importlib.simple
import importlib.util
import io
import os
import pathlib
import sys
import tempfile
import warnings
import zipfile

# ---------------------------------------------------------------------------
# 1. Submodule bindings & identity
# ---------------------------------------------------------------------------
print(
    "identities:",
    importlib.abc is sys.modules["importlib.abc"],
    importlib.resources is sys.modules["importlib.resources"],
    importlib.metadata is sys.modules["importlib.metadata"],
    importlib.readers is sys.modules["importlib.readers"],
    importlib.simple is sys.modules["importlib.simple"],
    importlib.resources.abc is sys.modules["importlib.resources.abc"],
    importlib.resources.readers is sys.modules["importlib.resources.readers"],
    importlib.resources.simple is sys.modules["importlib.resources.simple"],
    importlib.readers.FileReader is importlib.resources.readers.FileReader,
    importlib.readers.MultiplexedPath is importlib.resources.readers.MultiplexedPath,
    importlib.simple.SimpleReader is importlib.resources.simple.SimpleReader,
    importlib.simple.TraversableReader is importlib.resources.simple.TraversableReader,
)

# ---------------------------------------------------------------------------
# 2. importlib.abc hierarchy, virtual registrations & abstract enforcement
# ---------------------------------------------------------------------------
print(
    "abc_subclass:",
    issubclass(importlib.abc.ResourceLoader, importlib.abc.Loader),
    issubclass(importlib.abc.InspectLoader, importlib.abc.Loader),
    issubclass(importlib.abc.ExecutionLoader, importlib.abc.InspectLoader),
    issubclass(importlib.abc.FileLoader, importlib.abc.ResourceLoader),
    issubclass(importlib.abc.FileLoader, importlib.abc.ExecutionLoader),
    issubclass(importlib.abc.SourceLoader, importlib.abc.ResourceLoader),
    issubclass(importlib.abc.SourceLoader, importlib.abc.ExecutionLoader),
    issubclass(importlib.machinery.PathFinder, importlib.abc.MetaPathFinder),
    issubclass(importlib.machinery.FileFinder, importlib.abc.PathEntryFinder),
    issubclass(importlib.machinery.BuiltinImporter, importlib.abc.MetaPathFinder),
    issubclass(importlib.machinery.SourceFileLoader, importlib.abc.SourceLoader),
    issubclass(importlib.machinery.SourceFileLoader, importlib.abc.FileLoader),
)

for abstract_cls in [
    importlib.abc.ResourceLoader,
    importlib.abc.InspectLoader,
    importlib.abc.ExecutionLoader,
    importlib.abc.SourceLoader,
    importlib.resources.abc.ResourceReader,
    importlib.resources.abc.TraversableResources,
]:
    try:
        abstract_cls()
        print("unexpected_inst:", abstract_cls.__name__)
    except TypeError:
        print("abstract_blocked:", abstract_cls.__name__)


class InMemorySourceLoader(importlib.abc.SourceLoader):
    def __init__(self, files_map):
        self._files = files_map

    def get_filename(self, fullname):
        return f"<mem:{fullname}>"

    def get_data(self, path):
        if path in self._files:
            return self._files[path]
        raise OSError(f"missing {path}")


mem_loader = InMemorySourceLoader({"<mem:demo_abc>": b"GREETING = 'hello_abc'\n"})
demo_spec = importlib.util.spec_from_loader("demo_abc", mem_loader)
demo_mod = importlib.util.module_from_spec(demo_spec)
mem_loader.exec_module(demo_mod)
print(
    "abc_source_loader:",
    isinstance(mem_loader, importlib.abc.SourceLoader),
    isinstance(mem_loader, importlib.abc.Loader),
    demo_mod.GREETING,
    mem_loader.get_source("demo_abc").strip(),
)

# ---------------------------------------------------------------------------
# 3. importlib.resources: files, as_file, compat funcs, namespace & simple
# ---------------------------------------------------------------------------
with tempfile.TemporaryDirectory() as tmpdir:
    # Regular package with nested resources
    pkg = os.path.join(tmpdir, "respkg3244")
    sub = os.path.join(pkg, "sub")
    os.makedirs(sub)
    with open(os.path.join(pkg, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("# pkg\n")
    with open(os.path.join(pkg, "hello.txt"), "w", encoding="utf-8") as f:
        f.write("Hello resources!")
    with open(os.path.join(sub, "data.bin"), "wb") as f:
        f.write(b"\x00\x01\x02\xff")

    # Multi-directory PEP 420 namespace package with resources
    ns_r1 = os.path.join(tmpdir, "ns_r1")
    ns_r2 = os.path.join(tmpdir, "ns_r2")
    os.makedirs(os.path.join(ns_r1, "nsres3244"))
    os.makedirs(os.path.join(ns_r2, "nsres3244"))
    with open(
        os.path.join(ns_r1, "nsres3244", "part1.txt"), "w", encoding="utf-8"
    ) as f:
        f.write("one")
    with open(
        os.path.join(ns_r2, "nsres3244", "part2.txt"), "w", encoding="utf-8"
    ) as f:
        f.write("two")

    # Zip archive for ZipReader
    zip_path = os.path.join(tmpdir, "bundle.zip")
    with zipfile.ZipFile(zip_path, "w") as zf:
        zf.writestr("zpkg/__init__.py", "")
        zf.writestr("zpkg/msg.txt", "from-zip")

    sys.path.insert(0, ns_r2)
    sys.path.insert(0, ns_r1)
    sys.path.insert(0, tmpdir)
    try:
        root = importlib.resources.files("respkg3244")
        f_txt = root / "hello.txt"
        f_bin = root.joinpath("sub", "data.bin")
        print(
            "res_files:",
            root.name,
            root.is_dir(),
            root.is_file(),
            f_txt.name,
            f_txt.is_file(),
            f_txt.read_text(encoding="utf-8"),
            f_bin.read_bytes(),
            "hello.txt" in [p.name for p in root.iterdir()],
            "sub" in [p.name for p in root.iterdir()],
        )

        with f_txt.open("r", encoding="utf-8") as fh:
            print("res_open_text:", fh.read())

        with importlib.resources.as_file(f_txt) as fs_path:
            print(
                "res_as_file_disk:",
                isinstance(fs_path, pathlib.Path),
                fs_path.is_file(),
                fs_path.read_text(encoding="utf-8"),
            )

        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            print(
                "res_compat:",
                importlib.resources.is_resource("respkg3244", "hello.txt"),
                importlib.resources.is_resource("respkg3244", "sub"),
                importlib.resources.read_text("respkg3244", "hello.txt"),
                importlib.resources.read_binary("respkg3244", "hello.txt"),
                "hello.txt" in list(importlib.resources.contents("respkg3244")),
            )
            with importlib.resources.open_binary("respkg3244", "hello.txt") as bf:
                print("res_open_bin:", bf.read())
            with importlib.resources.open_text("respkg3244", "hello.txt") as tf:
                print("res_open_txt:", tf.read())
            with importlib.resources.path("respkg3244", "hello.txt") as p:
                print("res_path_ctx:", isinstance(p, pathlib.Path), p.read_text())

        # Namespace package resources (MultiplexedPath)
        ns_root = importlib.resources.files("nsres3244")
        print(
            "res_ns:",
            isinstance(ns_root, importlib.resources.readers.MultiplexedPath),
            ns_root.name,
            ns_root.is_dir(),
            (ns_root / "part1.txt").read_text(encoding="utf-8"),
            (ns_root / "part2.txt").read_text(encoding="utf-8"),
            sorted(p.name for p in ns_root.iterdir()),
        )

        # FileReader and ZipReader
        class FakeFileLoader:
            def __init__(self, p):
                self.path = p

        freader = importlib.resources.readers.FileReader(
            FakeFileLoader(os.path.join(pkg, "__init__.py"))
        )
        print(
            "res_file_reader:",
            (freader.files() / "hello.txt").read_text(encoding="utf-8"),
            freader.is_resource("hello.txt"),
            freader.is_resource("missing.txt"),
        )

        class FakeZipLoader:
            def __init__(self, archive, prefix=""):
                self.archive = archive
                self.prefix = prefix

        zreader = importlib.resources.readers.ZipReader(
            FakeZipLoader(zip_path, ""), "zpkg"
        )
        zroot = zreader.files()
        print(
            "res_zip_reader:",
            zreader.is_resource("msg.txt"),
            zreader.is_resource("nope.txt"),
            (zroot / "msg.txt").read_text(encoding="utf-8"),
            (zroot / "msg.txt").read_bytes(),
        )
        with importlib.resources.as_file(zroot / "msg.txt") as zp:
            saved_zp = zp
            print("res_zip_as_file:", isinstance(zp, pathlib.Path), zp.read_text())
        print("res_zip_cleaned:", not saved_zp.exists())
        if hasattr(zroot, "root"):
            zroot.root.close()
        del zroot, zreader

        # Custom Traversable + as_file temp materialization
        class MemFileTraversable(importlib.resources.abc.Traversable):
            def __init__(self, name, payload):
                self._name = name
                self._payload = payload

            @property
            def name(self):
                return self._name

            def is_file(self):
                return True

            def is_dir(self):
                return False

            def iterdir(self):
                return iter(())

            def open(self, mode="r", *args, **kwargs):
                if "b" in mode:
                    return io.BytesIO(self._payload)
                encoding = kwargs.get("encoding") or "utf-8"
                return io.StringIO(self._payload.decode(encoding))

        shandle = MemFileTraversable("note.txt", b"in-memory")
        print(
            "res_mem_trav:",
            isinstance(shandle, importlib.resources.abc.Traversable),
            shandle.name,
            shandle.is_file(),
            shandle.is_dir(),
            shandle.read_bytes(),
            shandle.read_text(encoding="utf-8"),
            issubclass(
                importlib.resources.simple.TraversableReader,
                importlib.resources.simple.SimpleReader,
            ),
            issubclass(
                importlib.resources.simple.TraversableReader,
                importlib.resources.abc.TraversableResources,
            ),
        )
        with importlib.resources.as_file(shandle) as tmp_p:
            saved_tmp = tmp_p
            print(
                "res_as_file_mem:",
                isinstance(tmp_p, pathlib.Path),
                tmp_p.is_file(),
                tmp_p.read_bytes(),
            )
        print("res_as_file_cleaned:", not saved_tmp.exists())
    finally:
        sys.path.remove(tmpdir)
        sys.path.remove(ns_r1)
        sys.path.remove(ns_r2)
        for k in ["respkg3244", "nsres3244"]:
            sys.modules.pop(k, None)

# ---------------------------------------------------------------------------
# 4. importlib.metadata against temporary .dist-info and .egg-info on sys.path
# ---------------------------------------------------------------------------
with tempfile.TemporaryDirectory() as tmpdir:
    # Build a realistic .dist-info package
    di = os.path.join(tmpdir, "sample_meta_pkg-2.4.1.dist-info")
    os.makedirs(di)
    with open(os.path.join(di, "METADATA"), "w", encoding="utf-8") as f:
        f.write(
            "Metadata-Version: 2.1\n"
            "Name: Sample-Meta_Pkg\n"
            "Version: 2.4.1\n"
            "Summary: Example metadata package\n"
            "Classifier: Programming Language :: Python :: 3\n"
            "Classifier: License :: OSI Approved :: MIT License\n"
            "Requires-Dist: alpha>=1.0\n"
            "Requires-Dist: beta; extra == 'dev'\n"
            "\n"
            "Detailed package description.\n"
        )
    with open(os.path.join(di, "entry_points.txt"), "w", encoding="utf-8") as f:
        f.write(
            "[console_scripts]\n"
            "sample-cli = sample_meta_pkg:run_cli\n"
            "\n"
            "[sample.plugins]\n"
            "greeter = sample_meta_pkg:Greeter.hello [fast, v2]\n"
        )
    with open(os.path.join(di, "RECORD"), "w", encoding="utf-8") as f:
        f.write(
            "sample_meta_pkg/__init__.py,sha256=deadbeef,68\n"
            "sample_meta_pkg-2.4.1.dist-info/METADATA,,\n"
        )
    with open(os.path.join(di, "top_level.txt"), "w", encoding="utf-8") as f:
        f.write("sample_meta_pkg\n")

    pkg_dir = os.path.join(tmpdir, "sample_meta_pkg")
    os.makedirs(pkg_dir)
    with open(os.path.join(pkg_dir, "__init__.py"), "w", encoding="utf-8") as f:
        f.write(
            "def run_cli():\n"
            "    return 'ran_cli'\n"
            "class Greeter:\n"
            "    @staticmethod\n"
            "    def hello():\n"
            "        return 'hi_plugin'\n"
        )

    # Also build an .egg-info package
    ei = os.path.join(tmpdir, "legacy_egg-0.9.0.egg-info")
    os.makedirs(ei)
    with open(os.path.join(ei, "PKG-INFO"), "w", encoding="utf-8") as f:
        f.write("Metadata-Version: 1.1\nName: legacy-egg\nVersion: 0.9.0\n")
    with open(os.path.join(ei, "top_level.txt"), "w", encoding="utf-8") as f:
        f.write("legacy_egg\n")

    sys.path.insert(0, tmpdir)
    try:
        # Distribution lookup & PEP 503 normalization
        dist = importlib.metadata.distribution("sample.meta-pkg")
        print(
            "meta_dist:",
            dist.name,
            dist.version,
            importlib.metadata.version("SAMPLE_META_PKG"),
            importlib.metadata.version("legacy_egg"),
        )

        # PackageNotFoundError
        try:
            importlib.metadata.distribution("no_such_dist_3244_xyz")
        except importlib.metadata.PackageNotFoundError as e:
            print(
                "meta_not_found:",
                isinstance(e, ModuleNotFoundError),
                e.name,
                str(e),
            )

        # PackageMetadata mapping protocol & JSON
        meta = importlib.metadata.metadata("sample-meta-pkg")
        print(
            "meta_map:",
            meta["Name"],
            meta["name"],
            meta.get("Summary"),
            meta.get("Missing", "fallback"),
            meta.get_all("Classifier"),
            meta.get_all("Missing"),
            "Version" in meta,
            "version" in meta,
            "Missing" in meta,
            len(meta),
            list(meta),
        )
        j = meta.json
        print(
            "meta_json:",
            j["name"],
            j["version"],
            j["summary"],
            j["classifier"],
            j["requires_dist"],
            j["description"],
        )

        # requires
        print("meta_requires:", importlib.metadata.requires("sample-meta-pkg"))

        # files & PackagePath & FileHash
        pkg_files = importlib.metadata.files("sample-meta-pkg")
        f0, f1 = pkg_files[0], pkg_files[1]
        print(
            "meta_files:",
            len(pkg_files),
            str(f0),
            f0.name,
            f0.hash.mode,
            f0.hash.value,
            repr(f0.hash),
            f0.size,
            f1.hash,
            f1.size,
            f0.locate() == dist.locate_file(f0),
            "run_cli" in f0.read_text(),
            b"run_cli" in f0.read_binary(),
        )

        # EntryPoint & EntryPoints & load()
        eps = importlib.metadata.entry_points(group="sample.plugins")
        ep_g = eps["greeter"]
        print(
            "meta_ep:",
            repr(ep_g),
            ep_g.name,
            ep_g.value,
            ep_g.group,
            ep_g.module,
            ep_g.attr,
            ep_g.extras,
            ep_g.matches(group="sample.plugins", name="greeter"),
            ep_g.matches(name="other"),
            eps.names,
            eps.groups,
        )
        loaded_fn = ep_g.load()
        print("meta_ep_load:", loaded_fn())

        cli_eps = dist.entry_points.select(group="console_scripts")
        print("meta_cli_load:", cli_eps["sample-cli"].load()())

        try:
            _ = eps["nonexistent"]
        except KeyError as e:
            print("meta_ep_missing:", type(e).__name__)

        # distributions(path=...) & Distribution.at(...)
        local_dists = list(importlib.metadata.distributions(path=[tmpdir]))
        print("meta_discover:", sorted(d.name for d in local_dists))
        at_dist = importlib.metadata.Distribution.at(di)
        print("meta_at:", at_dist.name, at_dist.version)

        # packages_distributions()
        pd = importlib.metadata.packages_distributions()
        print(
            "meta_pkg_dists:",
            pd.get("sample_meta_pkg"),
            pd.get("legacy_egg"),
        )
    finally:
        sys.path.remove(tmpdir)
        sys.modules.pop("sample_meta_pkg", None)
