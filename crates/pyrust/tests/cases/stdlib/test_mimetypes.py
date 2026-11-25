"""CPython 3.12 parity tests for the `mimetypes` module (issue #3211)."""

import io
import os
import pathlib
import tempfile
import mimetypes


# 1. Default built-in tables via MimeTypes() (platform-independent)
db = mimetypes.MimeTypes()

cases = [
    ("index.html", True, ("text/html", None)),
    ("index.htm", True, ("text/html", None)),
    ("INDEX.HTML", True, ("text/html", None)),
    ("readme.txt", True, ("text/plain", None)),
    ("data.json", True, ("application/json", None)),
    ("feed.xml", True, ("text/xml", None)),
    ("table.csv", True, ("text/csv", None)),
    ("style.css", True, ("text/css", None)),
    ("app.js", True, ("text/javascript", None)),
    ("mod.mjs", True, ("text/javascript", None)),
    ("logo.png", True, ("image/png", None)),
    ("photo.jpg", True, ("image/jpeg", None)),
    ("photo.jpeg", True, ("image/jpeg", None)),
    ("anim.gif", True, ("image/gif", None)),
    ("icon.svg", True, ("image/svg+xml", None)),
    ("pic.webp", True, (None, None)),
    ("pic.webp", False, ("image/webp", None)),
    ("doc.pdf", True, ("application/pdf", None)),
    ("archive.zip", True, ("application/zip", None)),
    ("archive.tar", True, ("application/x-tar", None)),
    ("archive.tar.gz", True, ("application/x-tar", "gzip")),
    ("archive.tgz", True, ("application/x-tar", "gzip")),
    ("archive.tar.bz2", True, ("application/x-tar", "bzip2")),
    ("archive.tbz2", True, ("application/x-tar", "bzip2")),
    ("archive.tar.xz", True, ("application/x-tar", "xz")),
    ("archive.txz", True, ("application/x-tar", "xz")),
    ("vector.svgz", True, ("image/svg+xml", "gzip")),
    ("old.txt.Z", True, ("text/plain", "compress")),
    ("old.txt.z", True, (None, None)),
    ("song.mp3", True, ("audio/mpeg", None)),
    ("movie.mp4", True, ("video/mp4", None)),
    ("mod.wasm", True, ("application/wasm", None)),
    ("script.py", True, ("text/x-python", None)),
    ("doc.rtf", True, (None, None)),
    ("doc.rtf", False, ("application/rtf", None)),
    ("no_extension", True, (None, None)),
    (".hidden", True, (None, None)),
]

for filename, strict, expected in cases:
    got = db.guess_type(filename, strict=strict)
    assert got == expected, (filename, strict, got, expected)
print("db_guess_type_ok", len(cases))

# 2. URLs, data: URLs, and pathlib.Path (os.fspath)
url_cases = [
    ("https://example.com/assets/app.js?v=123#top", ("text/javascript", None)),
    ("http://example.com/files/archive.tar.gz", ("application/x-tar", "gzip")),
    ("file:///tmp/photo.PNG", ("image/png", None)),
    ("data:text/plain;base64,SGVsbG8=", ("text/plain", None)),
    ("data:image/png;base64,iVBORw0KGgo=", ("image/png", None)),
    ("data:,Hello%2C%20World%21", ("text/plain", None)),
    ("data:charset=utf-8,hello", ("text/plain", None)),
    ("data:malformed_no_comma", (None, None)),
]
for url, expected in url_cases:
    got = db.guess_type(url)
    assert got == expected, (url, got, expected)

assert db.guess_type(pathlib.Path("dir/report.pdf")) == ("application/pdf", None)
print("urls_and_paths_ok", True)

# 3. guess_extension & guess_all_extensions
assert db.guess_extension("text/html") == ".html"
assert db.guess_all_extensions("text/html") == [".html", ".htm"]
assert db.guess_extension("TEXT/HTML") == ".html"
assert db.guess_extension("application/json") == ".json"
assert db.guess_extension("image/jpeg") == ".jpg"
assert db.guess_all_extensions("image/jpeg") == [".jpg", ".jpe", ".jpeg"]
assert db.guess_extension("image/webp", strict=True) is None
assert db.guess_extension("image/webp", strict=False) == ".webp"
assert db.guess_extension("application/x-unknown-zzz") is None
assert db.guess_all_extensions("application/x-unknown-zzz") == []
print("guess_extensions_ok", True)

# 4. add_type on MimeTypes instance
db.add_type("application/x-custom", ".cst")
assert db.guess_type("file.cst") == ("application/x-custom", None)
assert db.guess_extension("application/x-custom") == ".cst"
db.add_type("application/x-custom", ".cst2")
assert db.guess_all_extensions("application/x-custom") == [".cst", ".cst2"]
db.add_type("application/x-nonstd", ".nstd", strict=False)
assert db.guess_type("file.nstd", strict=True) == (None, None)
assert db.guess_type("file.nstd", strict=False) == ("application/x-nonstd", None)
print("add_type_ok", True)

# 5. readfp & read_mime_types & MimeTypes(filenames=...)
sample_mime = (
    "# Comment line\n"
    "application/x-pyrust   pyr pyrs # inline comment\n"
    "\n"
    "text/x-demo            dmo\n"
)
db2 = mimetypes.MimeTypes()
db2.readfp(io.StringIO(sample_mime))
assert db2.guess_type("hello.pyr") == ("application/x-pyrust", None)
assert db2.guess_type("hello.pyrs") == ("application/x-pyrust", None)
assert db2.guess_type("hello.dmo") == ("text/x-demo", None)
assert db2.guess_all_extensions("application/x-pyrust") == [".pyr", ".pyrs"]

with tempfile.TemporaryDirectory() as tmpdir:
    mf = os.path.join(tmpdir, "custom.mime.types")
    with open(mf, "w", encoding="utf-8") as f:
        f.write(sample_mime)
    parsed_map = mimetypes.read_mime_types(mf)
    assert parsed_map[".pyr"] == "application/x-pyrust"
    assert parsed_map[".dmo"] == "text/x-demo"
    db3 = mimetypes.MimeTypes(filenames=(mf,))
    assert db3.guess_type("test.pyr") == ("application/x-pyrust", None)
    assert mimetypes.read_mime_types(os.path.join(tmpdir, "missing.types")) is None

print("readfp_and_files_ok", True)

# 6. Module-level API & attributes
assert isinstance(mimetypes.knownfiles, list)
assert isinstance(mimetypes.types_map, dict)
assert isinstance(mimetypes.common_types, dict)
assert isinstance(mimetypes.encodings_map, dict)
assert isinstance(mimetypes.suffix_map, dict)
assert mimetypes.encodings_map[".gz"] == "gzip"
assert mimetypes.suffix_map[".tgz"] == ".tar.gz"

assert mimetypes.guess_type("page.html") == ("text/html", None)
assert mimetypes.guess_type("notes.txt") == ("text/plain", None)
assert mimetypes.guess_type("data.json") == ("application/json", None)
assert mimetypes.guess_type("photo.png") == ("image/png", None)
assert mimetypes.guess_type("doc.pdf") == ("application/pdf", None)
assert mimetypes.guess_type("archive.tar.gz") == ("application/x-tar", "gzip")
assert mimetypes.inited is True

mimetypes.add_type("application/x-pyrust-global", ".pyrg")
assert mimetypes.guess_type("a.pyrg") == ("application/x-pyrust-global", None)
assert mimetypes.guess_extension("application/x-pyrust-global") == ".pyrg"
assert mimetypes.guess_all_extensions("application/x-pyrust-global") == [".pyrg"]
print("module_api_ok", True)

print("ALL MIMETYPES TESTS PASSED")
