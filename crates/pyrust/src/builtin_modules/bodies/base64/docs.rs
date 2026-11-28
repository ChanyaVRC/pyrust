// Docstrings of CPython 3.12.3 `Lib/base64.py` (generated from the
// reference interpreter; `None` where the source has no docstring).

pub(super) const MODULE_DOC: &str =
    r####"Base16, Base32, Base64 (RFC 3548), Base85 and Ascii85 data encodings"####;

pub(super) const _B32_ENCODE_DOCSTRING: &str = r####"
Encode the bytes-like objects using {encoding} and return a bytes object.
"####;

pub(super) const _B32_DECODE_DOCSTRING: &str = r####"
Decode the {encoding} encoded bytes-like object or ASCII string s.

Optional casefold is a flag specifying whether a lowercase alphabet is
acceptable as input.  For security purposes, the default is False.
{extra_args}
The result is returned as a bytes object.  A binascii.Error is raised if
the input is incorrectly padded or if there are non-alphabet
characters present in the input.
"####;

pub(super) const _B32_DECODE_MAP01_DOCSTRING: &str = r####"
RFC 3548 allows for optional mapping of the digit 0 (zero) to the
letter O (oh), and for optional mapping of the digit 1 (one) to
either the letter I (eye) or letter L (el).  The optional argument
map01 when not None, specifies which letter the digit 1 should be
mapped to (when map01 is not None, the digit 0 is always mapped to
the letter O).  For security purposes the default is None, so that
0 and 1 are not allowed in the input.
"####;

/// The docstring of the `base64` function `name`.
pub(super) fn function_doc(name: &str) -> Option<Option<&'static str>> {
    Some(match name {
        "_bytes_from_decode_data" => None,
        "b64encode" => Some(
            r####"Encode the bytes-like object s using Base64 and return a bytes object.

    Optional altchars should be a byte string of length 2 which specifies an
    alternative alphabet for the '+' and '/' characters.  This allows an
    application to e.g. generate url or filesystem safe Base64 strings.
    "####,
        ),
        "b64decode" => Some(
            r####"Decode the Base64 encoded bytes-like object or ASCII string s.

    Optional altchars must be a bytes-like object or ASCII string of length 2
    which specifies the alternative alphabet used instead of the '+' and '/'
    characters.

    The result is returned as a bytes object.  A binascii.Error is raised if
    s is incorrectly padded.

    If validate is False (the default), characters that are neither in the
    normal base-64 alphabet nor the alternative alphabet are discarded prior
    to the padding check.  If validate is True, these non-alphabet characters
    in the input result in a binascii.Error.
    For more information about the strict base64 check, see:

    https://docs.python.org/3.11/library/binascii.html#binascii.a2b_base64
    "####,
        ),
        "standard_b64encode" => Some(
            r####"Encode bytes-like object s using the standard Base64 alphabet.

    The result is returned as a bytes object.
    "####,
        ),
        "standard_b64decode" => Some(
            r####"Decode bytes encoded with the standard Base64 alphabet.

    Argument s is a bytes-like object or ASCII string to decode.  The result
    is returned as a bytes object.  A binascii.Error is raised if the input
    is incorrectly padded.  Characters that are not in the standard alphabet
    are discarded prior to the padding check.
    "####,
        ),
        "urlsafe_b64encode" => Some(
            r####"Encode bytes using the URL- and filesystem-safe Base64 alphabet.

    Argument s is a bytes-like object to encode.  The result is returned as a
    bytes object.  The alphabet uses '-' instead of '+' and '_' instead of
    '/'.
    "####,
        ),
        "urlsafe_b64decode" => Some(
            r####"Decode bytes using the URL- and filesystem-safe Base64 alphabet.

    Argument s is a bytes-like object or ASCII string to decode.  The result
    is returned as a bytes object.  A binascii.Error is raised if the input
    is incorrectly padded.  Characters that are not in the URL-safe base-64
    alphabet, and are not a plus '+' or slash '/', are discarded prior to the
    padding check.

    The alphabet uses '-' instead of '+' and '_' instead of '/'.
    "####,
        ),
        "_b32encode" => None,
        "_b32decode" => None,
        "b32encode" => Some(
            r####"
Encode the bytes-like objects using base32 and return a bytes object.
"####,
        ),
        "b32decode" => Some(
            r####"
Decode the base32 encoded bytes-like object or ASCII string s.

Optional casefold is a flag specifying whether a lowercase alphabet is
acceptable as input.  For security purposes, the default is False.

RFC 3548 allows for optional mapping of the digit 0 (zero) to the
letter O (oh), and for optional mapping of the digit 1 (one) to
either the letter I (eye) or letter L (el).  The optional argument
map01 when not None, specifies which letter the digit 1 should be
mapped to (when map01 is not None, the digit 0 is always mapped to
the letter O).  For security purposes the default is None, so that
0 and 1 are not allowed in the input.

The result is returned as a bytes object.  A binascii.Error is raised if
the input is incorrectly padded or if there are non-alphabet
characters present in the input.
"####,
        ),
        "b32hexencode" => Some(
            r####"
Encode the bytes-like objects using base32hex and return a bytes object.
"####,
        ),
        "b32hexdecode" => Some(
            r####"
Decode the base32hex encoded bytes-like object or ASCII string s.

Optional casefold is a flag specifying whether a lowercase alphabet is
acceptable as input.  For security purposes, the default is False.

The result is returned as a bytes object.  A binascii.Error is raised if
the input is incorrectly padded or if there are non-alphabet
characters present in the input.
"####,
        ),
        "b16encode" => Some(
            r####"Encode the bytes-like object s using Base16 and return a bytes object.
    "####,
        ),
        "b16decode" => Some(
            r####"Decode the Base16 encoded bytes-like object or ASCII string s.

    Optional casefold is a flag specifying whether a lowercase alphabet is
    acceptable as input.  For security purposes, the default is False.

    The result is returned as a bytes object.  A binascii.Error is raised if
    s is incorrectly padded or if there are non-alphabet characters present
    in the input.
    "####,
        ),
        "_85encode" => None,
        "a85encode" => Some(
            r####"Encode bytes-like object b using Ascii85 and return a bytes object.

    foldspaces is an optional flag that uses the special short sequence 'y'
    instead of 4 consecutive spaces (ASCII 0x20) as supported by 'btoa'. This
    feature is not supported by the "standard" Adobe encoding.

    wrapcol controls whether the output should have newline (b'\n') characters
    added to it. If this is non-zero, each output line will be at most this
    many characters long.

    pad controls whether the input is padded to a multiple of 4 before
    encoding. Note that the btoa implementation always pads.

    adobe controls whether the encoded byte sequence is framed with <~ and ~>,
    which is used by the Adobe implementation.
    "####,
        ),
        "a85decode" => Some(
            r####"Decode the Ascii85 encoded bytes-like object or ASCII string b.

    foldspaces is a flag that specifies whether the 'y' short sequence should be
    accepted as shorthand for 4 consecutive spaces (ASCII 0x20). This feature is
    not supported by the "standard" Adobe encoding.

    adobe controls whether the input sequence is in Adobe Ascii85 format (i.e.
    is framed with <~ and ~>).

    ignorechars should be a byte string containing characters to ignore from the
    input. This should only contain whitespace characters, and by default
    contains all whitespace characters in ASCII.

    The result is returned as a bytes object.
    "####,
        ),
        "b85encode" => Some(
            r####"Encode bytes-like object b in base85 format and return a bytes object.

    If pad is true, the input is padded with b'\0' so its length is a multiple of
    4 bytes before encoding.
    "####,
        ),
        "b85decode" => Some(
            r####"Decode the base85-encoded bytes-like object or ASCII string b

    The result is returned as a bytes object.
    "####,
        ),
        "encode" => Some(r####"Encode a file; input and output are binary files."####),
        "decode" => Some(r####"Decode a file; input and output are binary files."####),
        "_input_type_check" => None,
        "encodebytes" => Some(
            r####"Encode a bytestring into a bytes object containing multiple lines
    of base-64 data."####,
        ),
        "decodebytes" => Some(r####"Decode a bytestring of base-64 data into a bytes object."####),
        "main" => Some(r####"Small main program"####),
        _ => return None,
    })
}
