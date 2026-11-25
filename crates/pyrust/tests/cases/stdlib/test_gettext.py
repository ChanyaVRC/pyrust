import builtins
import gettext
import io
import os
import struct
import tempfile


def build_mo(entries, endian="<"):
    """Build a valid GNU `.mo` binary catalog from a dict of bytes->bytes."""
    keys = sorted(entries.keys())
    count = len(keys)
    # Header: 7 * 4 = 28 bytes (magic, version, count, orig_off, trans_off, hash_size, hash_off)
    orig_table_off = 28
    trans_table_off = orig_table_off + count * 8
    strings_off = trans_table_off + count * 8

    orig_entries = []
    trans_entries = []
    blob = bytearray()

    for k in keys:
        orig_entries.append((len(k), strings_off + len(blob)))
        blob.extend(k)
        blob.append(0)
    for k in keys:
        v = entries[k]
        trans_entries.append((len(v), strings_off + len(blob)))
        blob.extend(v)
        blob.append(0)

    magic = 0x950412DE
    header = struct.pack(
        endian + "7I",
        magic,
        0,
        count,
        orig_table_off,
        trans_table_off,
        0,
        0,
    )
    otable = b"".join(struct.pack(endian + "2I", l, o) for l, o in orig_entries)
    ttable = b"".join(struct.pack(endian + "2I", l, o) for l, o in trans_entries)
    return header + otable + ttable + bytes(blob)


# 1. Module exports
print("all:", sorted(gettext.__all__))
print("Catalog is translation:", gettext.Catalog is gettext.translation)

# 2. c2py plural expression compiler
plural_cases = [
    ("germanic", "n != 1"),
    ("french", "n > 1"),
    ("japanese", "0"),
    ("polish", "n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2"),
    ("russian", "(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)"),
    ("unary_not", "!(n == 1)"),
    ("arith", "(n + 1) / 2 - 1"),
]
sample_ns = (0, 1, 2, 3, 4, 5, 11, 12, 14, 21, 22, 25, 101)
for label, expr in plural_cases:
    fn = gettext.c2py(expr)
    vals = [fn(n) for n in sample_ns]
    print("c2py", label, vals, all(type(v) is int for v in vals))

for bad_expr in ("__import__(1)", "n = 1", "n + a", "n ** 2", "1 ? 2", "(n + 1", ""):
    try:
        gettext.c2py(bad_expr)
        print("UNEXPECTED OK:", bad_expr)
    except ValueError:
        print("c2py bad -> ValueError:", repr(bad_expr))

# 3. NullTranslations
nt = gettext.NullTranslations()
print("null gettext:", nt.gettext("Hello"))
print("null ngettext:", nt.ngettext("one", "many", 1), nt.ngettext("one", "many", 2), nt.ngettext("one", "many", 0))
print("null pgettext:", nt.pgettext("menu", "File"))
print("null npgettext:", nt.npgettext("menu", "item", "items", 1), nt.npgettext("menu", "item", "items", 3))
print("null info/charset:", nt.info(), nt.charset())

# 4. GNUTranslations (both Little-Endian and Big-Endian .mo files)
slavic_header = (
    b"Project-Id-Version: Demo 1.0\n"
    b"Content-Type: text/plain; charset=UTF-8\n"
    b"Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\n"
)
catalog_entries = {
    b"": slavic_header,
    b"Hello": "Привет".encode("utf-8"),
    b"menu\x04Open": "Открыть".encode("utf-8"),
    b"status\x04Open": "Открыто".encode("utf-8"),
    b"apple\x00apples": "\x00".join(["яблоко", "яблока", "яблок"]).encode("utf-8"),
    b"cart\x04item\x00items": "\x00".join(["товар", "товара", "товаров"]).encode("utf-8"),
}

for endian_name, endian in (("LE", "<"), ("BE", ">")):
    mo_bytes = build_mo(catalog_entries, endian=endian)
    gt = gettext.GNUTranslations(io.BytesIO(mo_bytes))
    print(endian_name, "charset:", gt.charset(), "proj:", gt.info().get("project-id-version"))
    print(endian_name, "gettext:", gt.gettext("Hello"), gt.gettext("Missing"))
    print(endian_name, "pgettext:", gt.pgettext("menu", "Open"), gt.pgettext("status", "Open"), gt.pgettext("other", "Open"))
    print(endian_name, "ngettext:", [gt.ngettext("apple", "apples", n) for n in (0, 1, 2, 5, 11, 21, 22)])
    print(endian_name, "npgettext:", [gt.npgettext("cart", "item", "items", n) for n in (1, 2, 5)])
    print(endian_name, "ngettext miss:", gt.ngettext("cat", "cats", 1), gt.ngettext("cat", "cats", 3))

# Corrupt / bad magic / bad version .mo files
try:
    gettext.GNUTranslations(io.BytesIO(b""))
except struct.error:
    print("empty mo -> struct.error")
for bad_mo in (b"BADMAGIC" + b"\x00" * 20, struct.pack("<7I", 0x950412DE, 2 << 16, 0, 28, 28, 0, 0)):
    try:
        gettext.GNUTranslations(io.BytesIO(bad_mo))
    except OSError:
        print("bad mo -> OSError")

# 5. Fallback chaining
primary_mo = build_mo({
    b"": b"Content-Type: text/plain; charset=UTF-8\n",
    b"greeting": "Bonjour (FR-CA)".encode("utf-8"),
})
secondary_mo = build_mo({
    b"": b"Content-Type: text/plain; charset=UTF-8\n",
    b"greeting": "Bonjour (FR)".encode("utf-8"),
    b"farewell": "Au revoir".encode("utf-8"),
    b"nav\x04Back": "Retour".encode("utf-8"),
    b"day\x00days": "jour\x00jours".encode("utf-8"),
})
t1 = gettext.GNUTranslations(io.BytesIO(primary_mo))
t2 = gettext.GNUTranslations(io.BytesIO(secondary_mo))
t1.add_fallback(t2)
print("fallback chain:", t1.gettext("greeting"), t1.gettext("farewell"), t1.gettext("unknown"))
print("fallback pgettext:", t1.pgettext("nav", "Back"), t1.ngettext("day", "days", 1), t1.ngettext("day", "days", 2))

# 6. find(), translation(), install(), bindtextdomain(), textdomain(), d*gettext()
saved_domain = gettext.textdomain()
saved_env = {k: os.environ.get(k) for k in ("LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG")}
saved_builtins = {k: builtins.__dict__.get(k) for k in ("_", "gettext", "ngettext", "pgettext", "npgettext")}
try:
    with tempfile.TemporaryDirectory() as tmpdir:
        for lang, payload in (("fr_CA", primary_mo), ("fr", secondary_mo)):
            lc_dir = os.path.join(tmpdir, lang, "LC_MESSAGES")
            os.makedirs(lc_dir)
            with open(os.path.join(lc_dir, "demo.mo"), "wb") as f:
                f.write(payload)

        found_one = gettext.find("demo", localedir=tmpdir, languages=["fr_CA"])
        found_all = gettext.find("demo", localedir=tmpdir, languages=["fr_CA"], all=True)
        print("find one exists:", found_one is not None and found_one.endswith("demo.mo"))
        print("find all count:", len(found_all))
        print("find missing:", gettext.find("nope", localedir=tmpdir, languages=["fr_CA"]))

        tr = gettext.translation("demo", localedir=tmpdir, languages=["fr_CA"])
        print("translation chain:", tr.gettext("greeting"), tr.gettext("farewell"))

        try:
            gettext.translation("nope", localedir=tmpdir, languages=["fr_CA"])
        except FileNotFoundError:
            print("translation missing -> FileNotFoundError")

        fb = gettext.translation("nope", localedir=tmpdir, languages=["fr_CA"], fallback=True)
        print("translation fallback:", type(fb).__name__, fb.gettext("hello"))

        # Global API with LANGUAGE env var
        os.environ["LANGUAGE"] = "fr_CA"
        print("bindtextdomain:", gettext.bindtextdomain("demo", tmpdir) == tmpdir)
        print("textdomain:", gettext.textdomain("demo"))
        print("global gettext:", gettext.gettext("greeting"), gettext.gettext("farewell"))
        print("global dgettext:", gettext.dgettext("demo", "greeting"), gettext.dgettext("missing_dom", "greeting"))
        print("global ngettext:", gettext.ngettext("day", "days", 1), gettext.ngettext("day", "days", 4))
        print("global dngettext:", gettext.dngettext("demo", "day", "days", 2))
        print("global pgettext:", gettext.pgettext("nav", "Back"))
        print("global dpgettext:", gettext.dpgettext("demo", "nav", "Back"))
        print("global npgettext:", gettext.npgettext("nav", "x", "xs", 1), gettext.npgettext("nav", "x", "xs", 2))
        print("global dnpgettext:", gettext.dnpgettext("demo", "nav", "x", "xs", 2))

        # install() into builtins
        gettext.install("demo", localedir=tmpdir, names=("ngettext", "pgettext", "npgettext"))
        print("installed _:", builtins._("greeting"))
        print("installed ngettext:", builtins.ngettext("day", "days", 2))
        print("installed pgettext:", builtins.pgettext("nav", "Back"))
finally:
    gettext.textdomain(saved_domain)
    for k, v in saved_env.items():
        if v is None:
            if k in os.environ:
                del os.environ[k]
        else:
            os.environ[k] = v
    for k, v in saved_builtins.items():
        if v is None:
            builtins.__dict__.pop(k, None)
        else:
            builtins.__dict__[k] = v
