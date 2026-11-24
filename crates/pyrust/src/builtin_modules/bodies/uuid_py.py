r"""UUID objects (universally unique identifiers) according to RFC 4122.

Pure-Python port of CPython 3.12's ``Lib/uuid.py`` for PyRust (issue #3185).
"""

import os
import time
from enum import Enum


RESERVED_NCS, RFC_4122, RESERVED_MICROSOFT, RESERVED_FUTURE = [
    'reserved for NCS compatibility',
    'specified in RFC 4122',
    'reserved for Microsoft compatibility',
    'reserved for future definition',
]

int_ = int      # The built-in int type
bytes_ = bytes  # The built-in bytes type


class SafeUUID(Enum):
    safe = 0
    unsafe = -1
    unknown = None


class UUID:
    """Instances of the UUID class represent UUIDs as specified in RFC 4122."""

    __slots__ = ('int', 'is_safe', '__weakref__')

    def __init__(self, hex=None, bytes=None, bytes_le=None, fields=None,
                 int=None, version=None, *, is_safe=SafeUUID.unknown):
        if [hex, bytes, bytes_le, fields, int].count(None) != 4:
            raise TypeError('one of the hex, bytes, bytes_le, fields, '
                            'or int arguments must be given')
        if hex is not None:
            hex = hex.replace('urn:', '').replace('uuid:', '')
            hex = hex.strip('{}').replace('-', '')
            if len(hex) != 32:
                raise ValueError('badly formed hexadecimal UUID string')
            int = int_(hex, 16)
        if bytes_le is not None:
            if len(bytes_le) != 16:
                raise ValueError('bytes_le is not a 16-char string')
            bytes = (bytes_le[4 - 1::-1] + bytes_le[6 - 1:4 - 1:-1] +
                     bytes_le[8 - 1:6 - 1:-1] + bytes_le[8:])
        if bytes is not None:
            if len(bytes) != 16:
                raise ValueError('bytes is not a 16-char string')
            assert isinstance(bytes, bytes_), repr(bytes)
            int = int_.from_bytes(bytes, 'big')
        if fields is not None:
            if len(fields) != 6:
                raise ValueError('fields is not a 6-tuple')
            (time_low, time_mid, time_hi_version,
             clock_seq_hi_variant, clock_seq_low, node) = fields
            if not 0 <= time_low < (1 << 32):
                raise ValueError('field 1 out of range (need a 32-bit value)')
            if not 0 <= time_mid < (1 << 16):
                raise ValueError('field 2 out of range (need a 16-bit value)')
            if not 0 <= time_hi_version < (1 << 16):
                raise ValueError('field 3 out of range (need a 16-bit value)')
            if not 0 <= clock_seq_hi_variant < (1 << 8):
                raise ValueError('field 4 out of range (need an 8-bit value)')
            if not 0 <= clock_seq_low < (1 << 8):
                raise ValueError('field 5 out of range (need an 8-bit value)')
            if not 0 <= node < (1 << 48):
                raise ValueError('field 6 out of range (need a 48-bit value)')
            clock_seq = (clock_seq_hi_variant << 8) | clock_seq_low
            int = ((time_low << 96) | (time_mid << 80) |
                   (time_hi_version << 64) | (clock_seq << 48) | node)
        if int is not None:
            if not 0 <= int < (1 << 128):
                raise ValueError('int is out of range (need a 128-bit value)')
        if version is not None:
            if not 1 <= version <= 5:
                raise ValueError('illegal version number')
            # Set the variant to RFC 4122.
            int &= ~(0xc000 << 48)
            int |= 0x8000 << 48
            # Set the version number.
            int &= ~(0xf000 << 64)
            int |= version << 76
        object.__setattr__(self, 'int', int)
        object.__setattr__(self, 'is_safe', is_safe)

    def __getstate__(self):
        d = {'int': self.int}
        if self.is_safe != SafeUUID.unknown:
            d['is_safe'] = self.is_safe.value
        return d

    def __setstate__(self, state):
        object.__setattr__(self, 'int', state['int'])
        object.__setattr__(
            self,
            'is_safe',
            SafeUUID(state['is_safe']) if 'is_safe' in state else SafeUUID.unknown,
        )

    def __eq__(self, other):
        if isinstance(other, UUID):
            return self.int == other.int
        return NotImplemented

    def __lt__(self, other):
        if isinstance(other, UUID):
            return self.int < other.int
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, UUID):
            return self.int > other.int
        return NotImplemented

    def __le__(self, other):
        if isinstance(other, UUID):
            return self.int <= other.int
        return NotImplemented

    def __ge__(self, other):
        if isinstance(other, UUID):
            return self.int >= other.int
        return NotImplemented

    def __hash__(self):
        return hash(self.int)

    def __int__(self):
        return self.int

    def __repr__(self):
        return '%s(%r)' % (self.__class__.__name__, str(self))

    def __setattr__(self, name, value):
        raise TypeError('UUID objects are immutable')

    def __str__(self):
        hex = '%032x' % self.int
        return '%s-%s-%s-%s-%s' % (
            hex[:8], hex[8:12], hex[12:16], hex[16:20], hex[20:],
        )

    @property
    def bytes(self):
        return self.int.to_bytes(16, 'big')

    @property
    def bytes_le(self):
        bytes = self.bytes
        return (bytes[4 - 1::-1] + bytes[6 - 1:4 - 1:-1] +
                bytes[8 - 1:6 - 1:-1] + bytes[8:])

    @property
    def fields(self):
        return (self.time_low, self.time_mid, self.time_hi_version,
                self.clock_seq_hi_variant, self.clock_seq_low, self.node)

    @property
    def time_low(self):
        return self.int >> 96

    @property
    def time_mid(self):
        return (self.int >> 80) & 0xffff

    @property
    def time_hi_version(self):
        return (self.int >> 64) & 0xffff

    @property
    def clock_seq_hi_variant(self):
        return (self.int >> 56) & 0xff

    @property
    def clock_seq_low(self):
        return (self.int >> 48) & 0xff

    @property
    def time(self):
        return (((self.time_hi_version & 0x0fff) << 48) |
                (self.time_mid << 32) | self.time_low)

    @property
    def clock_seq(self):
        return (((self.clock_seq_hi_variant & 0x3f) << 8) |
                self.clock_seq_low)

    @property
    def node(self):
        return self.int & 0xffffffffffff

    @property
    def hex(self):
        return '%032x' % self.int

    @property
    def urn(self):
        return 'urn:uuid:' + str(self)

    @property
    def variant(self):
        if not self.int & (0x8000 << 48):
            return RESERVED_NCS
        elif not self.int & (0x4000 << 48):
            return RFC_4122
        elif not self.int & (0x2000 << 48):
            return RESERVED_MICROSOFT
        else:
            return RESERVED_FUTURE

    @property
    def version(self):
        # The version bits are only meaningful for RFC 4122 UUIDs.
        if self.variant == RFC_4122:
            return int((self.int >> 76) & 0xf)


_node = None


def _random_getnode():
    """Get a random 48-bit node ID with the RFC 4122 multicast bit set."""
    return int_.from_bytes(os.urandom(6), 'big') | (1 << 40)


def getnode():
    """Get the hardware address as a 48-bit positive integer.

    Falls back to a cached random 48-bit number with the multicast bit set
    as recommended in RFC 4122 section 4.5.
    """
    global _node
    if _node is not None:
        return _node
    _node = _random_getnode()
    return _node


_last_timestamp = None


def uuid1(node=None, clock_seq=None):
    """Generate a UUID from a host ID, sequence number, and the current time."""
    global _last_timestamp
    nanoseconds = time.time_ns()
    # 0x01b21dd213814000 is the number of 100-ns intervals between the
    # UUID epoch 1582-10-15 00:00:00 and the Unix epoch 1970-01-01 00:00:00.
    timestamp = nanoseconds // 100 + 0x01b21dd213814000
    if _last_timestamp is not None and timestamp <= _last_timestamp:
        timestamp = _last_timestamp + 1
    _last_timestamp = timestamp
    if clock_seq is None:
        clock_seq = int_.from_bytes(os.urandom(2), 'big') & 0x3fff
    time_low = timestamp & 0xffffffff
    time_mid = (timestamp >> 32) & 0xffff
    time_hi_version = (timestamp >> 48) & 0x0fff
    clock_seq_low = clock_seq & 0xff
    clock_seq_hi_variant = (clock_seq >> 8) & 0x3f
    if node is None:
        node = getnode()
    return UUID(
        fields=(time_low, time_mid, time_hi_version,
                clock_seq_hi_variant, clock_seq_low, node),
        version=1,
    )


def uuid3(namespace, name):
    """Generate a UUID from the MD5 hash of a namespace UUID and a name."""
    if isinstance(name, str):
        name = bytes(name, 'utf-8')
    from hashlib import md5
    digest = md5(namespace.bytes + name, usedforsecurity=False).digest()
    return UUID(bytes=digest[:16], version=3)


def uuid4():
    """Generate a random UUID."""
    return UUID(bytes=os.urandom(16), version=4)


def uuid5(namespace, name):
    """Generate a UUID from the SHA-1 hash of a namespace UUID and a name."""
    if isinstance(name, str):
        name = bytes(name, 'utf-8')
    from hashlib import sha1
    digest = sha1(namespace.bytes + name).digest()
    return UUID(bytes=digest[:16], version=5)


# Standard namespaces for use with uuid3() or uuid5().
NAMESPACE_DNS = UUID('6ba7b810-9dad-11d1-80b4-00c04fd430c8')
NAMESPACE_URL = UUID('6ba7b811-9dad-11d1-80b4-00c04fd430c8')
NAMESPACE_OID = UUID('6ba7b812-9dad-11d1-80b4-00c04fd430c8')
NAMESPACE_X500 = UUID('6ba7b814-9dad-11d1-80b4-00c04fd430c8')
