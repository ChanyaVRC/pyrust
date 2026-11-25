"""Guess the MIME type of a file (CPython 3.12 parity, issue #3211)."""

import os
import sys
import urllib.parse

__all__ = [
    "knownfiles",
    "inited",
    "MimeTypes",
    "guess_type",
    "guess_all_extensions",
    "guess_extension",
    "add_type",
    "init",
    "read_mime_types",
    "suffix_map",
    "encodings_map",
    "types_map",
    "common_types",
]

knownfiles = [
    "/etc/mime.types",
    "/etc/httpd/mime.types",
    "/etc/httpd/conf/mime.types",
    "/etc/apache/mime.types",
    "/etc/apache2/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/lib/netscape/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/etc/mime.types",
]

inited = False
_db = None


def _posix_splitext(p):
    """POSIX-style splitext matching posixpath.splitext (slash separator only)."""
    sep_index = p.rfind("/")
    dot_index = p.rfind(".")
    if dot_index > sep_index:
        filename_index = sep_index + 1
        while filename_index < dot_index:
            if p[filename_index] != ".":
                return p[:dot_index], p[dot_index:]
            filename_index += 1
    return p, ""


def _sync_module_globals():
    """Keep the live module attributes synchronized after init()."""
    mod = sys.modules.get("mimetypes")
    if mod is not None:
        mod.inited = inited
        mod.encodings_map = encodings_map
        mod.suffix_map = suffix_map
        mod.types_map = types_map
        mod.common_types = common_types


class MimeTypes:
    """MIME-types datastore."""

    def __init__(self, filenames=(), strict=True):
        if not inited:
            init()
        self.encodings_map = _encodings_map_default.copy()
        self.suffix_map = _suffix_map_default.copy()
        self.types_map = ({}, {})  # (non-strict, strict)
        self.types_map_inv = ({}, {})
        for ext, ctype in _types_map_default.items():
            self.add_type(ctype, ext, True)
        for ext, ctype in _common_types_default.items():
            self.add_type(ctype, ext, False)
        for name in filenames:
            self.read(name, strict)

    def add_type(self, type, ext, strict=True):
        """Add a mapping between a MIME type and a file extension."""
        self.types_map[strict][ext] = type
        exts = self.types_map_inv[strict].setdefault(type, [])
        if ext not in exts:
            exts.append(ext)

    def guess_type(self, url, strict=True):
        """Guess the MIME type and encoding of a file URL or path-like object."""
        url = os.fspath(url)
        p = urllib.parse.urlparse(url)
        if p.scheme and len(p.scheme) > 1:
            scheme = p.scheme
            url = p.path
        else:
            scheme = None
            url = os.path.splitdrive(url)[1]
        if scheme == "data":
            comma = url.find(",")
            if comma < 0:
                return None, None
            semi = url.find(";", 0, comma)
            if semi >= 0:
                ctype = url[:semi]
            else:
                ctype = url[:comma]
            if "=" in ctype or "/" not in ctype:
                ctype = "text/plain"
            return ctype, None
        base, ext = _posix_splitext(url)
        while (ext_lower := ext.lower()) in self.suffix_map:
            base, ext = _posix_splitext(base + self.suffix_map[ext_lower])
        if ext in self.encodings_map:
            encoding = self.encodings_map[ext]
            base, ext = _posix_splitext(base)
        else:
            encoding = None
        ext = ext.lower()
        tmap = self.types_map[True]
        if ext in tmap:
            return tmap[ext], encoding
        if strict:
            return None, encoding
        tmap = self.types_map[False]
        if ext in tmap:
            return tmap[ext], encoding
        return None, encoding

    def guess_all_extensions(self, type, strict=True):
        """Guess all extensions for a file based on its MIME type."""
        type = type.lower()
        extensions = list(self.types_map_inv[True].get(type, []))
        if not strict:
            for ext in self.types_map_inv[False].get(type, []):
                if ext not in extensions:
                    extensions.append(ext)
        return extensions

    def guess_extension(self, type, strict=True):
        """Guess the preferred extension for a file based on its MIME type."""
        extensions = self.guess_all_extensions(type, strict)
        if not extensions:
            return None
        return extensions[0]

    def read(self, filename, strict=True):
        """Read a single mime.types-format file specified by pathname."""
        with open(filename, encoding="utf-8") as fp:
            self.readfp(fp, strict)

    def readfp(self, fp, strict=True):
        """Read a single mime.types-format file object."""
        while True:
            line = fp.readline()
            if not line:
                break
            words = line.split()
            for i in range(len(words)):
                if words[i][0] == "#":
                    del words[i:]
                    break
            if not words:
                continue
            ctype, suffixes = words[0], words[1:]
            for suff in suffixes:
                self.add_type(ctype, "." + suff, strict)

    def read_windows_registry(self, strict=True):
        """No-op: PyRust uses deterministic built-in MIME tables."""
        return None


def guess_type(url, strict=True):
    """Guess the MIME type and encoding of a URL or path-like object."""
    if _db is None:
        init()
    return _db.guess_type(url, strict)


def guess_all_extensions(type, strict=True):
    """Guess all extensions for a given MIME type."""
    if _db is None:
        init()
    return _db.guess_all_extensions(type, strict)


def guess_extension(type, strict=True):
    """Guess the preferred extension for a given MIME type."""
    if _db is None:
        init()
    return _db.guess_extension(type, strict)


def add_type(type, ext, strict=True):
    """Add a mapping between a MIME type and an extension."""
    if _db is None:
        init()
    return _db.add_type(type, ext, strict)


def init(files=None):
    """Initialize the module-level MIME type database."""
    global suffix_map, types_map, encodings_map, common_types
    global inited, _db
    inited = True

    if files is None or _db is None:
        db = MimeTypes()
        read_files = list(files) if files is not None else []
    else:
        db = _db
        read_files = list(files)

    for fname in read_files:
        if os.path.isfile(fname):
            db.read(fname)

    encodings_map = db.encodings_map
    suffix_map = db.suffix_map
    types_map = db.types_map[True]
    common_types = db.types_map[False]
    _db = db
    _sync_module_globals()


def read_mime_types(file):
    """Parse one mime.types file and return a dict of mappings, or None."""
    try:
        f = open(file, encoding="utf-8")
    except OSError:
        return None
    with f:
        db = MimeTypes()
        db.readfp(f, True)
        return db.types_map[True]


def _default_mime_types():
    global suffix_map, _suffix_map_default
    global encodings_map, _encodings_map_default
    global types_map, _types_map_default
    global common_types, _common_types_default

    suffix_map = _suffix_map_default = {
        ".svgz": ".svg.gz",
        ".tgz": ".tar.gz",
        ".taz": ".tar.gz",
        ".tz": ".tar.gz",
        ".tbz2": ".tar.bz2",
        ".txz": ".tar.xz",
    }

    encodings_map = _encodings_map_default = {
        ".gz": "gzip",
        ".Z": "compress",
        ".bz2": "bzip2",
        ".xz": "xz",
        ".br": "br",
    }

    types_map = _types_map_default = {
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".json": "application/json",
        ".webmanifest": "application/manifest+json",
        ".doc": "application/msword",
        ".dot": "application/msword",
        ".wiz": "application/msword",
        ".nq": "application/n-quads",
        ".nt": "application/n-triples",
        ".bin": "application/octet-stream",
        ".a": "application/octet-stream",
        ".dll": "application/octet-stream",
        ".exe": "application/octet-stream",
        ".o": "application/octet-stream",
        ".obj": "application/octet-stream",
        ".so": "application/octet-stream",
        ".oda": "application/oda",
        ".pdf": "application/pdf",
        ".p7c": "application/pkcs7-mime",
        ".ps": "application/postscript",
        ".ai": "application/postscript",
        ".eps": "application/postscript",
        ".trig": "application/trig",
        ".m3u": "application/vnd.apple.mpegurl",
        ".m3u8": "application/vnd.apple.mpegurl",
        ".xls": "application/vnd.ms-excel",
        ".xlb": "application/vnd.ms-excel",
        ".ppt": "application/vnd.ms-powerpoint",
        ".pot": "application/vnd.ms-powerpoint",
        ".ppa": "application/vnd.ms-powerpoint",
        ".pps": "application/vnd.ms-powerpoint",
        ".pwz": "application/vnd.ms-powerpoint",
        ".wasm": "application/wasm",
        ".bcpio": "application/x-bcpio",
        ".cpio": "application/x-cpio",
        ".csh": "application/x-csh",
        ".dvi": "application/x-dvi",
        ".gtar": "application/x-gtar",
        ".hdf": "application/x-hdf",
        ".h5": "application/x-hdf5",
        ".latex": "application/x-latex",
        ".mif": "application/x-mif",
        ".cdf": "application/x-netcdf",
        ".nc": "application/x-netcdf",
        ".p12": "application/x-pkcs12",
        ".pfx": "application/x-pkcs12",
        ".ram": "application/x-pn-realaudio",
        ".pyc": "application/x-python-code",
        ".pyo": "application/x-python-code",
        ".sh": "application/x-sh",
        ".shar": "application/x-shar",
        ".swf": "application/x-shockwave-flash",
        ".sv4cpio": "application/x-sv4cpio",
        ".sv4crc": "application/x-sv4crc",
        ".tar": "application/x-tar",
        ".tcl": "application/x-tcl",
        ".tex": "application/x-tex",
        ".texi": "application/x-texinfo",
        ".texinfo": "application/x-texinfo",
        ".roff": "application/x-troff",
        ".t": "application/x-troff",
        ".tr": "application/x-troff",
        ".man": "application/x-troff-man",
        ".me": "application/x-troff-me",
        ".ms": "application/x-troff-ms",
        ".ustar": "application/x-ustar",
        ".src": "application/x-wais-source",
        ".xsl": "application/xml",
        ".rdf": "application/xml",
        ".wsdl": "application/xml",
        ".xpdl": "application/xml",
        ".zip": "application/zip",
        ".3gp": "audio/3gpp",
        ".3gpp": "audio/3gpp",
        ".3g2": "audio/3gpp2",
        ".3gpp2": "audio/3gpp2",
        ".aac": "audio/aac",
        ".adts": "audio/aac",
        ".loas": "audio/aac",
        ".ass": "audio/aac",
        ".au": "audio/basic",
        ".snd": "audio/basic",
        ".mp3": "audio/mpeg",
        ".mp2": "audio/mpeg",
        ".opus": "audio/opus",
        ".aif": "audio/x-aiff",
        ".aifc": "audio/x-aiff",
        ".aiff": "audio/x-aiff",
        ".ra": "audio/x-pn-realaudio",
        ".wav": "audio/x-wav",
        ".avif": "image/avif",
        ".bmp": "image/bmp",
        ".gif": "image/gif",
        ".ief": "image/ief",
        ".jpg": "image/jpeg",
        ".jpe": "image/jpeg",
        ".jpeg": "image/jpeg",
        ".heic": "image/heic",
        ".heif": "image/heif",
        ".png": "image/png",
        ".svg": "image/svg+xml",
        ".tiff": "image/tiff",
        ".tif": "image/tiff",
        ".ico": "image/vnd.microsoft.icon",
        ".ras": "image/x-cmu-raster",
        ".pnm": "image/x-portable-anymap",
        ".pbm": "image/x-portable-bitmap",
        ".pgm": "image/x-portable-graymap",
        ".ppm": "image/x-portable-pixmap",
        ".rgb": "image/x-rgb",
        ".xbm": "image/x-xbitmap",
        ".xpm": "image/x-xpixmap",
        ".xwd": "image/x-xwindowdump",
        ".eml": "message/rfc822",
        ".mht": "message/rfc822",
        ".mhtml": "message/rfc822",
        ".nws": "message/rfc822",
        ".css": "text/css",
        ".csv": "text/csv",
        ".html": "text/html",
        ".htm": "text/html",
        ".n3": "text/n3",
        ".txt": "text/plain",
        ".bat": "text/plain",
        ".c": "text/plain",
        ".h": "text/plain",
        ".ksh": "text/plain",
        ".pl": "text/plain",
        ".srt": "text/plain",
        ".rtx": "text/richtext",
        ".tsv": "text/tab-separated-values",
        ".vtt": "text/vtt",
        ".py": "text/x-python",
        ".etx": "text/x-setext",
        ".sgm": "text/x-sgml",
        ".sgml": "text/x-sgml",
        ".vcf": "text/x-vcard",
        ".xml": "text/xml",
        ".mp4": "video/mp4",
        ".mpeg": "video/mpeg",
        ".m1v": "video/mpeg",
        ".mpa": "video/mpeg",
        ".mpe": "video/mpeg",
        ".mpg": "video/mpeg",
        ".mov": "video/quicktime",
        ".qt": "video/quicktime",
        ".webm": "video/webm",
        ".avi": "video/x-msvideo",
        ".movie": "video/x-sgi-movie",
    }

    common_types = _common_types_default = {
        ".rtf": "application/rtf",
        ".midi": "audio/midi",
        ".mid": "audio/midi",
        ".jpg": "image/jpg",
        ".pict": "image/pict",
        ".pct": "image/pict",
        ".pic": "image/pict",
        ".webp": "image/webp",
        ".xul": "text/xul",
    }


_default_mime_types()
