# Python-level `UCD` class and `ucd_3_2_0` compatibility alias for
# `unicodedata` (issue #3203).

_SENTINEL = object()


class UCD:
    """Unicode Character Database wrapper class."""

    def __init__(self, version=unidata_version):
        self.unidata_version = version

    def category(self, chr):
        return category(chr)

    def bidirectional(self, chr):
        return bidirectional(chr)

    def combining(self, chr):
        return combining(chr)

    def east_asian_width(self, chr):
        return east_asian_width(chr)

    def mirrored(self, chr):
        return mirrored(chr)

    def decomposition(self, chr):
        return decomposition(chr)

    def decimal(self, chr, default=_SENTINEL):
        if default is _SENTINEL:
            return decimal(chr)
        return decimal(chr, default)

    def digit(self, chr, default=_SENTINEL):
        if default is _SENTINEL:
            return digit(chr)
        return digit(chr, default)

    def numeric(self, chr, default=_SENTINEL):
        if default is _SENTINEL:
            return numeric(chr)
        return numeric(chr, default)

    def name(self, chr, default=_SENTINEL):
        if default is _SENTINEL:
            return name(chr)
        return name(chr, default)

    def lookup(self, name_str):
        return lookup(name_str)

    def normalize(self, form, unistr):
        return normalize(form, unistr)

    def is_normalized(self, form, unistr):
        return is_normalized(form, unistr)


ucd_3_2_0 = UCD("3.2.0")
