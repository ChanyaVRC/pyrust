# Parity fixture for issue #1781: UnicodeDecodeError.object must include BOM bytes
# for utf-16 and utf-32 BOM-prefixed codec errors.
#
# CPython 3.12 always sets .object to the full original bytes passed to decode(),
# and adjusts .start/.end to account for the BOM prefix.  Before this fix,
# pyrust stripped the BOM before passing bytes to the decoder, so .object omitted
# the BOM and .start/.end were relative to the post-BOM slice.


def check(label, data, encoding):
    try:
        data.decode(encoding)
        print(f"{label}: no error")
    except UnicodeDecodeError as e:
        print(f"{label}: object={e.object!r} start={e.start} end={e.end} reason={e.reason!r}")


# UTF-16 LE BOM (ff fe) — lone high surrogate 0xD800 (LE: 00 d8)
check("utf16-le-bom-lone-high", b"\xff\xfe\x00\xd8", "utf-16")

# UTF-16 LE BOM — lone low surrogate 0xDC00 (LE: 00 dc)
check("utf16-le-bom-lone-low", b"\xff\xfe\x00\xdc", "utf-16")

# UTF-16 LE BOM — high surrogate followed by non-low-surrogate unit
check("utf16-le-bom-surrogate-nolow", b"\xff\xfe\x00\xd8\x41\x00", "utf-16")

# UTF-16 LE BOM — odd byte count after BOM (truncated)
check("utf16-le-bom-truncated", b"\xff\xfeA", "utf-16")

# UTF-16 LE BOM — valid data; must still decode correctly
check("utf16-le-bom-valid", b"\xff\xfe\x41\x00", "utf-16")

# UTF-16 BE BOM (fe ff) — lone high surrogate 0xD800 (BE: d8 00)
check("utf16-be-bom-lone-high", b"\xfe\xff\xd8\x00", "utf-16")

# utf-16-le / utf-16-be have no BOM — .object and offsets must be unaffected
check("utf16le-no-bom-lone-high", b"\x00\xd8", "utf-16-le")
check("utf16be-no-bom-lone-high", b"\xd8\x00", "utf-16-be")

# UTF-32 LE BOM (ff fe 00 00) — invalid codepoint 0x110000 (LE: 00 00 11 00)
check("utf32-le-bom-invalid", b"\xff\xfe\x00\x00\x00\x00\x11\x00", "utf-32")

# UTF-32 LE BOM — truncated (1 byte after BOM)
check("utf32-le-bom-truncated", b"\xff\xfe\x00\x00A", "utf-32")

# UTF-32 LE BOM — valid data
check("utf32-le-bom-valid", b"\xff\xfe\x00\x00\x41\x00\x00\x00", "utf-32")

# UTF-32 BE BOM (00 00 fe ff) — invalid codepoint 0x110000 (BE: 00 11 00 00)
check("utf32-be-bom-invalid", b"\x00\x00\xfe\xff\x00\x11\x00\x00", "utf-32")

# utf-32-le / utf-32-be have no BOM — must be unaffected
check("utf32le-no-bom-invalid", b"\x00\x00\x11\x00", "utf-32-le")
check("utf32be-no-bom-invalid", b"\x00\x11\x00\x00", "utf-32-be")
