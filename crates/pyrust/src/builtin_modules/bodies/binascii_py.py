"""
Python 3.12 ``binascii`` module wrapper backed by native primitives in
``binascii.rs`` (issue #3145).
"""

_UNSET = object()


class Error(ValueError):
    pass


class Incomplete(Exception):
    pass


def b2a_hex(data, sep=_UNSET, bytes_per_sep=1):
    """Hexadecimal representation of binary data."""
    if sep is _UNSET:
        return _b2a_hex(data, None, False, bytes_per_sep)
    return _b2a_hex(data, sep, True, bytes_per_sep)


def hexlify(data, sep=_UNSET, bytes_per_sep=1):
    """Hexadecimal representation of binary data."""
    if sep is _UNSET:
        return _b2a_hex(data, None, False, bytes_per_sep)
    return _b2a_hex(data, sep, True, bytes_per_sep)


def a2b_hex(hexstr, /):
    """Binary data of hexadecimal representation."""
    return _a2b_hex(Error, hexstr)


def unhexlify(hexstr, /):
    """Binary data of hexadecimal representation."""
    return _a2b_hex(Error, hexstr)


def b2a_base64(data, /, *, newline=True):
    """Base64-code line of data."""
    return _b2a_base64(data, newline)


def a2b_base64(data, /, *, strict_mode=False):
    """Decode a line of base64 data."""
    return _a2b_base64(Error, data, strict_mode)


def crc32(data, value=0, /):
    """Compute CRC-32 incrementally."""
    return _crc32(data, value)


def crc_hqx(data, value, /):
    """Compute CRC-CCITT incrementally."""
    return _crc_hqx(data, value)


def b2a_uu(data, /, *, backtick=False):
    """Uuencode line of data."""
    return _b2a_uu(Error, data, backtick)


def a2b_uu(data, /):
    """Decode a line of uuencoded data."""
    return _a2b_uu(Error, data)


def b2a_qp(data, quotetabs=False, istext=True, header=False):
    """Encode a string using quoted-printable encoding."""
    return _b2a_qp(data, quotetabs, istext, header)


def a2b_qp(data, header=False):
    """Decode a string of qp-encoded data."""
    return _a2b_qp(data, header)
