"""A fast, lightweight IPv4/IPv6 manipulation library in Python.

This library is used to create/poke/manipulate IPv4 and IPv6 addresses
and networks.
"""

import functools

IPV4LENGTH = 32
IPV6LENGTH = 128


class AddressValueError(ValueError):
    """A Value Error related to the address."""


class NetmaskValueError(ValueError):
    """A Value Error related to the netmask."""


def ip_address(address):
    """Take an IP string/int and return an object of the correct type."""
    try:
        return IPv4Address(address)
    except (AddressValueError, NetmaskValueError):
        pass

    try:
        return IPv6Address(address)
    except (AddressValueError, NetmaskValueError):
        pass

    raise ValueError(f"{address!r} does not appear to be an IPv4 or IPv6 address")


def ip_network(address, strict=True):
    """Take an IP string/int and return an object of the correct type."""
    try:
        return IPv4Network(address, strict)
    except (AddressValueError, NetmaskValueError):
        pass

    try:
        return IPv6Network(address, strict)
    except (AddressValueError, NetmaskValueError):
        pass

    raise ValueError(f"{address!r} does not appear to be an IPv4 or IPv6 network")


def ip_interface(address):
    """Take an IP string/int and return an object of the correct type."""
    try:
        return IPv4Interface(address)
    except (AddressValueError, NetmaskValueError):
        pass

    try:
        return IPv6Interface(address)
    except (AddressValueError, NetmaskValueError):
        pass

    raise ValueError(f"{address!r} does not appear to be an IPv4 or IPv6 interface")


def v4_int_to_packed(address):
    """Represent an address as 4 packed bytes in network (big-endian) order."""
    try:
        return address.to_bytes(4, "big")
    except OverflowError:
        raise ValueError("Address negative or too large for IPv4")


def v6_int_to_packed(address):
    """Represent an address as 16 packed bytes in network (big-endian) order."""
    try:
        return address.to_bytes(16, "big")
    except OverflowError:
        raise ValueError("Address negative or too large for IPv6")


def _split_optional_netmask(address):
    """Helper to split the netmask and raise AddressValueError if needed."""
    addr = str(address).split("/")
    if len(addr) > 2:
        raise AddressValueError(f"Only one '/' permitted in {address!r}")
    return addr


def _find_address_range(addresses):
    """Find a sequence of sorted deduplicated IPv#Address."""
    it = iter(addresses)
    first = last = next(it)
    for ip in it:
        if ip._ip != last._ip + 1:
            yield first, last
            first = ip
        last = ip
    yield first, last


def _count_righthand_zero_bits(number, bits):
    """Count the number of zero bits on the right hand side."""
    if number == 0:
        return bits
    return min(bits, (~number & (number - 1)).bit_length())


def summarize_address_range(first, last):
    """Summarize a network range given the first and last IP addresses."""
    if not (isinstance(first, _BaseAddress) and isinstance(last, _BaseAddress)):
        raise TypeError("first and last must be IP addresses, not networks")
    if first.version != last.version:
        raise TypeError(f"{first} and {last} are not of the same version")
    if first > last:
        raise ValueError("last IP address must be greater than first")

    if first.version == 4:
        ip = IPv4Network
    elif first.version == 6:
        ip = IPv6Network
    else:
        raise ValueError("unknown IP version")

    ip_bits = first._max_prefixlen
    first_int = first._ip
    last_int = last._ip
    while first_int <= last_int:
        nbits = min(
            _count_righthand_zero_bits(first_int, ip_bits),
            (last_int - first_int + 1).bit_length() - 1,
        )
        net = ip((first_int, ip_bits - nbits))
        yield net
        first_int += 1 << nbits
        if first_int - 1 == ip._ALL_ONES:
            break


def _collapse_addresses_internal(addresses):
    """Loops through the addresses, collapsing concurrent netblocks."""
    to_merge = list(addresses)
    subnets = {}
    while to_merge:
        net = to_merge.pop()
        supernet = net.supernet()
        existing = subnets.get(supernet)
        if existing is None:
            subnets[supernet] = net
        elif existing != net:
            del subnets[supernet]
            to_merge.append(supernet)
    last = None
    for net in sorted(subnets.values()):
        if last is not None:
            if last.broadcast_address >= net.broadcast_address:
                continue
        yield net
        last = net


def collapse_addresses(addresses):
    """Collapse a list of IP objects."""
    addrs = []
    ips = []
    nets = []

    for ip in addresses:
        if isinstance(ip, _BaseAddress):
            if ips and ips[-1]._version != ip._version:
                raise TypeError(f"{ip} and {ips[-1]} are not of the same version")
            ips.append(ip)
        elif ip._prefixlen == ip._max_prefixlen:
            if ips and ips[-1]._version != ip._version:
                raise TypeError(f"{ip} and {ips[-1]} are not of the same version")
            try:
                ips.append(ip.ip)
            except AttributeError:
                ips.append(ip.network_address)
        else:
            if nets and nets[-1]._version != ip._version:
                raise TypeError(f"{ip} and {nets[-1]} are not of the same version")
            nets.append(ip)

    ips = sorted(set(ips))

    if ips:
        for first, last in _find_address_range(ips):
            addrs.extend(summarize_address_range(first, last))

    return _collapse_addresses_internal(addrs + nets)


def get_mixed_type_key(obj):
    """Return a key suitable for sorting between networks and addresses."""
    if isinstance(obj, _BaseNetwork):
        return obj._get_networks_key()
    elif isinstance(obj, _BaseAddress):
        return obj._get_address_key()
    return NotImplemented


class _IPAddressBase:
    """The mother class."""

    __slots__ = ()

    @property
    def exploded(self):
        """Return the longhand version of the IP address as a string."""
        return self._explode_shorthand_ip_string()

    @property
    def compressed(self):
        """Return the shorthand version of the IP address as a string."""
        return str(self)

    @property
    def reverse_pointer(self):
        """The name of the reverse DNS pointer for the IP address."""
        return self._reverse_pointer()

    @property
    def version(self):
        msg = f"{type(self).__name__} has no version specified"
        raise NotImplementedError(msg)

    def _check_int_address(self, address):
        if address < 0:
            msg = f"{address!r} (< 0) is not permitted as an IPv{self._version} address"
            raise AddressValueError(msg)
        if address > self._ALL_ONES:
            msg = f"{address!r} (>= 2**{self._max_prefixlen}) is not permitted as an IPv{self._version} address"
            raise AddressValueError(msg)

    def _check_packed_address(self, address, expected_len):
        address_len = len(address)
        if address_len != expected_len:
            msg = f"{address!r} (len {address_len} != {expected_len}) is not permitted as an IPv{self._version} address"
            raise AddressValueError(msg)

    @classmethod
    def _ip_int_from_prefix(cls, prefixlen):
        """Turn the prefix length into a bitwise netmask."""
        return cls._ALL_ONES ^ (cls._ALL_ONES >> prefixlen)

    @classmethod
    def _prefix_from_ip_int(cls, ip_int):
        """Return prefix length from the bitwise netmask."""
        trailing_zeroes = _count_righthand_zero_bits(ip_int, cls._max_prefixlen)
        prefixlen = cls._max_prefixlen - trailing_zeroes
        leading_ones = ip_int >> trailing_zeroes
        all_ones = (1 << prefixlen) - 1
        if leading_ones != all_ones:
            byteslen = cls._max_prefixlen // 8
            details = ip_int.to_bytes(byteslen, "big")
            msg = f"Netmask pattern {details!r} mixes zeroes & ones"
            raise ValueError(msg)
        return prefixlen

    @classmethod
    def _report_invalid_netmask(cls, netmask_str):
        msg = f"{netmask_str!r} is not a valid netmask"
        raise NetmaskValueError(msg) from None

    @classmethod
    def _prefix_from_prefix_string(cls, prefixlen_str):
        """Return prefix length from a numeric string."""
        if not (prefixlen_str.isascii() and prefixlen_str.isdigit()):
            cls._report_invalid_netmask(prefixlen_str)
        try:
            prefixlen = int(prefixlen_str)
        except ValueError:
            cls._report_invalid_netmask(prefixlen_str)
        if not (0 <= prefixlen <= cls._max_prefixlen):
            cls._report_invalid_netmask(prefixlen_str)
        return prefixlen

    @classmethod
    def _prefix_from_ip_string(cls, ip_str):
        """Turn a netmask/hostmask string into a prefix length."""
        try:
            ip_int = cls._ip_int_from_string(ip_str)
        except AddressValueError:
            cls._report_invalid_netmask(ip_str)

        try:
            return cls._prefix_from_ip_int(ip_int)
        except ValueError:
            pass

        ip_int ^= cls._ALL_ONES
        try:
            return cls._prefix_from_ip_int(ip_int)
        except ValueError:
            cls._report_invalid_netmask(ip_str)

    @classmethod
    def _split_addr_prefix(cls, address):
        """Helper function to parse address of Network/Interface."""
        if isinstance(address, (bytes, int)):
            return address, cls._max_prefixlen

        if not isinstance(address, tuple):
            address = _split_optional_netmask(address)

        if len(address) > 1:
            return address
        return address[0], cls._max_prefixlen

    def __reduce__(self):
        return self.__class__, (str(self),)


@functools.total_ordering
class _BaseAddress(_IPAddressBase):
    """A generic IP object."""

    __slots__ = ()

    def __int__(self):
        return self._ip

    def __eq__(self, other):
        try:
            return self._ip == other._ip and self._version == other._version
        except AttributeError:
            return NotImplemented

    def __lt__(self, other):
        if not isinstance(other, _BaseAddress):
            return NotImplemented
        if self._version != other._version:
            raise TypeError(f"{self} and {other} are not of the same version")
        if self._ip != other._ip:
            return self._ip < other._ip
        return False

    def __add__(self, other):
        if not isinstance(other, int):
            return NotImplemented
        return self.__class__(int(self) + other)

    def __sub__(self, other):
        if not isinstance(other, int):
            return NotImplemented
        return self.__class__(int(self) - other)

    def __repr__(self):
        return f"{self.__class__.__name__}({str(self)!r})"

    def __str__(self):
        return str(self._string_from_ip_int(self._ip))

    def __hash__(self):
        return hash(hex(int(self._ip)))

    def _get_address_key(self):
        return (self._version, self)

    def __reduce__(self):
        return self.__class__, (self._ip,)

    def __format__(self, fmt):
        """Returns an IP address as a formatted string."""
        if not fmt or fmt[-1] == "s":
            return format(str(self), fmt)

        if fmt[-1] in "Xbxn":
            if fmt[-1] == "n":
                fmt = fmt[:-1] + ("b" if self._version == 4 else "x")

            # Format integer with optional '#' / '_' / width / fill / align.
            # Handle zero-padding width default when only presentation/prefix/grouping given.
            # Parse mini-language manually to match CPython 3.12's ipaddress.__format__:
            # [[fill]align][#][0][minimumwidth][_][type]
            orig_fmt = fmt
            fmt_type = fmt[-1]
            rest = fmt[:-1]

            grouping = ""
            if rest.endswith("_"):
                grouping = "_"
                rest = rest[:-1]

            fill = ""
            align = ""
            if len(rest) >= 2 and rest[1] in "<>=^":
                fill = rest[0]
                align = rest[1]
                rest = rest[2:]
            elif len(rest) >= 1 and rest[0] in "<>=^":
                align = rest[0]
                rest = rest[1:]

            alt = ""
            if rest.startswith("#"):
                alt = "#"
                rest = rest[1:]

            zero_pad = ""
            if rest.startswith("0"):
                zero_pad = "0"
                rest = rest[1:]

            width_str = rest
            if width_str and not width_str.isdigit():
                raise ValueError(f"Invalid format specifier {orig_fmt!r}")

            if not width_str and not align:
                # Default zero-padded full width for the IP bit length.
                bits = self._max_prefixlen
                if fmt_type == "b":
                    width = bits
                    if grouping:
                        width += (bits - 1) // 4
                else:
                    width = bits // 4
                    if grouping:
                        width += (width - 1) // 4
                if alt:
                    width += 2
                int_fmt = f"{alt}0{width}{grouping}{fmt_type}"
            else:
                int_fmt = f"{fill}{align}{alt}{zero_pad}{width_str}{grouping}{fmt_type}"

            return format(int(self), int_fmt)

        raise ValueError(f"Unknown format code {fmt[-1]!r} for object of type {type(self).__name__!r}")


@functools.total_ordering
class _BaseNetwork(_IPAddressBase):
    """A generic IP network object."""

    def __repr__(self):
        return f"{self.__class__.__name__}({str(self)!r})"

    def __str__(self):
        return f"{self.network_address}/{self.prefixlen}"

    def hosts(self):
        """Generate Iterator over usable hosts in a network."""
        network = int(self.network_address)
        broadcast = int(self.broadcast_address)
        for x in range(network + 1, broadcast):
            yield self._address_class(x)

    def __iter__(self):
        network = int(self.network_address)
        broadcast = int(self.broadcast_address)
        for x in range(network, broadcast + 1):
            yield self._address_class(x)

    def __getitem__(self, n):
        network = int(self.network_address)
        broadcast = int(self.broadcast_address)
        if n >= 0:
            if network + n > broadcast:
                raise IndexError("address out of range")
            return self._address_class(network + n)
        else:
            n += 1
            if broadcast + n < network:
                raise IndexError("address out of range")
            return self._address_class(broadcast + n)

    def __lt__(self, other):
        if not isinstance(other, _BaseNetwork):
            return NotImplemented
        if self._version != other._version:
            raise TypeError(f"{self} and {other} are not of the same version")
        if self.network_address != other.network_address:
            return self.network_address < other.network_address
        if self.netmask != other.netmask:
            return self.netmask < other.netmask
        return False

    def __eq__(self, other):
        try:
            return (
                self._version == other._version
                and self.network_address == other.network_address
                and int(self.netmask) == int(other.netmask)
            )
        except AttributeError:
            return NotImplemented

    def __hash__(self):
        return hash(int(self.network_address) ^ int(self.netmask))

    def __contains__(self, other):
        if self._version != other._version:
            return False
        if isinstance(other, _BaseNetwork):
            return False
        return (other._ip & self.netmask._ip) == self.network_address._ip

    def overlaps(self, other):
        """Tell if self is partly contained in other."""
        return (
            self.network_address in other
            or self.broadcast_address in other
            or other.network_address in self
            or other.broadcast_address in self
        )

    @property
    def broadcast_address(self):
        x = self._cache.get("broadcast_address")
        if x is None:
            x = self._address_class(int(self.network_address) | int(self.hostmask))
            self._cache["broadcast_address"] = x
        return x

    @property
    def hostmask(self):
        x = self._cache.get("hostmask")
        if x is None:
            x = self._address_class(int(self.netmask) ^ self._ALL_ONES)
            self._cache["hostmask"] = x
        return x

    @property
    def with_prefixlen(self):
        return f"{self.network_address}/{self._prefixlen}"

    @property
    def with_netmask(self):
        return f"{self.network_address}/{self.netmask}"

    @property
    def with_hostmask(self):
        return f"{self.network_address}/{self.hostmask}"

    @property
    def num_addresses(self):
        """Number of hosts in the current subnet."""
        return int(self.broadcast_address) - int(self.network_address) + 1

    @property
    def _address_class(self):
        msg = f"{type(self).__name__} has no associated address class"
        raise NotImplementedError(msg)

    @property
    def prefixlen(self):
        return self._prefixlen

    def address_exclude(self, other):
        """Remove an address from a larger block."""
        if not self._version == other._version:
            raise TypeError(f"{self} and {other} are not of the same version")

        if not isinstance(other, _BaseNetwork):
            raise TypeError(f"{other} is not a network object")

        if not other.subnet_of(self):
            raise ValueError(f"{other} not contained in {self}")
        if other == self:
            return

        other = other.__class__(f"{other.network_address}/{other.prefixlen}")

        s1, s2 = self.subnets()
        while s1 != other and s2 != other:
            if other.subnet_of(s1):
                yield s2
                s1, s2 = s1.subnets()
            elif other.subnet_of(s2):
                yield s1
                s1, s2 = s2.subnets()
            else:
                raise AssertionError(
                    f"Error performing exclusion: s1: {s1} s2: {s2} other: {other}"
                )
        if s1 == other:
            yield s2
        elif s2 == other:
            yield s1
        else:
            raise AssertionError(
                f"Error performing exclusion: s1: {s1} s2: {s2} other: {other}"
            )

    def compare_networks(self, other):
        """Compare two IP objects."""
        if self._version != other._version:
            raise TypeError(f"{self} and {other} are not of the same type")
        if self.network_address < other.network_address:
            return -1
        if self.network_address > other.network_address:
            return 1
        if self.netmask < other.netmask:
            return -1
        if self.netmask > other.netmask:
            return 1
        return 0

    def _get_networks_key(self):
        """Network-only key function."""
        return (self._version, self.network_address, self.netmask)

    def subnets(self, prefixlen_diff=1, new_prefix=None):
        """The subnets which join to make the current subnet."""
        if self._prefixlen == self._max_prefixlen:
            yield self
            return

        if new_prefix is not None:
            if new_prefix < self._prefixlen:
                raise ValueError("new prefix must be longer")
            if prefixlen_diff != 1:
                raise ValueError("cannot set prefixlen_diff and new_prefix")
            prefixlen_diff = new_prefix - self._prefixlen

        if prefixlen_diff < 0:
            raise ValueError("prefix length diff must be > 0")
        new_prefixlen = self._prefixlen + prefixlen_diff

        if new_prefixlen > self._max_prefixlen:
            raise ValueError(
                f"prefix length diff {prefixlen_diff!r} is invalid for netblock {self}"
            )

        start = int(self.network_address)
        end = int(self.broadcast_address) + 1
        step = (int(self.hostmask) + 1) >> prefixlen_diff
        for new_addr in range(start, end, step):
            current = self.__class__((new_addr, new_prefixlen))
            yield current

    def supernet(self, prefixlen_diff=1, new_prefix=None):
        """The supernet containing the current network."""
        if self._prefixlen == 0:
            return self

        if new_prefix is not None:
            if new_prefix > self._prefixlen:
                raise ValueError("new prefix must be shorter")
            if prefixlen_diff != 1:
                raise ValueError("cannot set prefixlen_diff and new_prefix")
            prefixlen_diff = self._prefixlen - new_prefix

        new_prefixlen = self.prefixlen - prefixlen_diff
        if new_prefixlen < 0:
            raise ValueError(
                f"current prefixlen is {self.prefixlen}, cannot have a prefixlen_diff of {prefixlen_diff}"
            )
        return self.__class__(
            (
                int(self.network_address) & (int(self.netmask) << prefixlen_diff),
                new_prefixlen,
            )
        )

    @property
    def is_multicast(self):
        return self.network_address.is_multicast and self.broadcast_address.is_multicast

    @staticmethod
    def _is_subnet_of(a, b):
        try:
            if a._version != b._version:
                raise TypeError(f"{a} and {b} are not of the same version")
            return (
                b.network_address <= a.network_address
                and b.broadcast_address >= a.broadcast_address
            )
        except AttributeError:
            raise TypeError(f"Unable to test subnet containment between {a} and {b}")

    def subnet_of(self, other):
        """Return True if this network is a subnet of other."""
        return self._is_subnet_of(self, other)

    def supernet_of(self, other):
        """Return True if this network is a supernet of other."""
        return self._is_subnet_of(other, self)

    @property
    def is_reserved(self):
        return self.network_address.is_reserved and self.broadcast_address.is_reserved

    @property
    def is_link_local(self):
        return self.network_address.is_link_local and self.broadcast_address.is_link_local

    @property
    def is_private(self):
        return any(
            self.network_address in priv_network
            and self.broadcast_address in priv_network
            for priv_network in self._constants._private_networks
        )

    @property
    def is_global(self):
        return not self.is_private

    @property
    def is_unspecified(self):
        return self.network_address.is_unspecified and self.broadcast_address.is_unspecified

    @property
    def is_loopback(self):
        return self.network_address.is_loopback and self.broadcast_address.is_loopback


class _BaseV4:
    """Base IPv4 object."""

    __slots__ = ()
    _version = 4
    _ALL_ONES = (2**IPV4LENGTH) - 1
    _max_prefixlen = IPV4LENGTH
    _netmask_cache = {}

    def _explode_shorthand_ip_string(self):
        return str(self)

    @classmethod
    def _make_netmask(cls, arg):
        """Make a (netmask, prefix_len) tuple from the given argument."""
        if arg not in cls._netmask_cache:
            if isinstance(arg, int):
                prefixlen = arg
                if not (0 <= prefixlen <= cls._max_prefixlen):
                    cls._report_invalid_netmask(prefixlen)
            else:
                try:
                    prefixlen = cls._prefix_from_prefix_string(arg)
                except NetmaskValueError:
                    prefixlen = cls._prefix_from_ip_string(arg)
            netmask = IPv4Address(cls._ip_int_from_prefix(prefixlen))
            cls._netmask_cache[arg] = netmask, prefixlen
        return cls._netmask_cache[arg]

    @classmethod
    def _ip_int_from_string(cls, ip_str):
        """Turn the given IP string into an integer for comparison."""
        if not ip_str:
            raise AddressValueError("Address cannot be empty")

        octets = ip_str.split(".")
        if len(octets) != 4:
            raise AddressValueError(f"Expected 4 octets in {ip_str!r}")

        try:
            return int.from_bytes(
                bytes(cls._parse_octet(octet) for octet in octets), "big"
            )
        except ValueError as exc:
            raise AddressValueError(f"{exc} in {ip_str!r}") from None

    @classmethod
    def _parse_octet(cls, octet_str):
        """Convert a decimal octet into an integer."""
        if not octet_str:
            raise ValueError("Empty octet not permitted")
        if not (octet_str.isascii() and octet_str.isdigit()):
            msg = f"Only decimal digits permitted in {octet_str!r}"
            raise ValueError(msg)
        if len(octet_str) > 3:
            msg = f"At most 3 characters permitted in {octet_str!r}"
            raise ValueError(msg)
        if octet_str != "0" and octet_str[0] == "0":
            msg = f"Leading zeros are not permitted in {octet_str!r}"
            raise ValueError(msg)
        octet_int = int(octet_str, 10)
        if octet_int > 255:
            raise ValueError(f"Octet {octet_int} (> 255) not permitted")
        return octet_int

    @classmethod
    def _string_from_ip_int(cls, ip_int):
        """Turns a 32-bit integer into dotted decimal notation."""
        return ".".join(str(b) for b in ip_int.to_bytes(4, "big"))

    def _reverse_pointer(self):
        """Return the reverse DNS pointer name for the IPv4 address."""
        reverse_octets = str(self).split(".")[::-1]
        return ".".join(reverse_octets) + ".in-addr.arpa"

    @property
    def max_prefixlen(self):
        return self._max_prefixlen

    @property
    def version(self):
        return self._version


class IPv4Address(_BaseV4, _BaseAddress):
    """Represent and manipulate single IPv4 Addresses."""

    __slots__ = ("_ip", "__weakref__")

    def __init__(self, address):
        if isinstance(address, int):
            self._check_int_address(address)
            self._ip = address
            return

        if isinstance(address, bytes):
            self._check_packed_address(address, 4)
            self._ip = int.from_bytes(address, "big")
            return

        if isinstance(address, IPv4Address):
            self._ip = address._ip
            return

        addr_str = str(address)
        if "/" in addr_str:
            raise AddressValueError(f"Unexpected '/' in {address!r}")
        self._ip = self._ip_int_from_string(addr_str)

    @property
    def packed(self):
        """The binary representation of this address."""
        return v4_int_to_packed(self._ip)

    @property
    def is_reserved(self):
        return self in self._constants._reserved_network

    @property
    def is_private(self):
        return any(self in net for net in self._constants._private_networks)

    @property
    def is_global(self):
        return self not in self._constants._public_network and not self.is_private

    @property
    def is_multicast(self):
        return self in self._constants._multicast_network

    @property
    def is_unspecified(self):
        return self == self._constants._unspecified_address

    @property
    def is_loopback(self):
        return self in self._constants._loopback_network

    @property
    def is_link_local(self):
        return self in self._constants._linklocal_network


class IPv4Interface(IPv4Address):
    def __init__(self, address):
        addr, mask = self._split_addr_prefix(address)
        IPv4Address.__init__(self, addr)
        self.network = IPv4Network((addr, mask), strict=False)
        self.netmask = self.network.netmask
        self._prefixlen = self.network._prefixlen

    @property
    def hostmask(self):
        return self.network.hostmask

    def __str__(self):
        return f"{self._string_from_ip_int(self._ip)}/{self._prefixlen}"

    def __eq__(self, other):
        address_equal = IPv4Address.__eq__(self, other)
        if address_equal is NotImplemented or not address_equal:
            return address_equal
        try:
            return self.network == other.network
        except AttributeError:
            return False

    def __lt__(self, other):
        address_less = IPv4Address.__lt__(self, other)
        if address_less is NotImplemented:
            return NotImplemented
        try:
            return self.network < other.network or (
                self.network == other.network and address_less
            )
        except AttributeError:
            return False

    def __hash__(self):
        return hash((self._ip, self._prefixlen, int(self.network.network_address)))

    __reduce__ = _IPAddressBase.__reduce__

    @property
    def ip(self):
        return IPv4Address(self._ip)

    @property
    def with_prefixlen(self):
        return f"{self._string_from_ip_int(self._ip)}/{self._prefixlen}"

    @property
    def with_netmask(self):
        return f"{self._string_from_ip_int(self._ip)}/{self.netmask}"

    @property
    def with_hostmask(self):
        return f"{self._string_from_ip_int(self._ip)}/{self.hostmask}"


class IPv4Network(_BaseV4, _BaseNetwork):
    """This class represents and manipulates 32-bit IPv4 network + addresses."""

    _address_class = IPv4Address

    def __init__(self, address, strict=True):
        self._cache = {}
        addr, mask = self._split_addr_prefix(address)

        self.network_address = IPv4Address(addr)
        self.netmask, self._prefixlen = self._make_netmask(mask)
        packed = int(self.network_address)
        if packed & int(self.netmask) != packed:
            if strict:
                raise ValueError(f"{self} has host bits set")
            else:
                self.network_address = IPv4Address(packed & int(self.netmask))

        if self._prefixlen >= (self._max_prefixlen - 1):
            self.hosts = self.__iter__

    @property
    def is_global(self):
        return (
            not (
                self.network_address in self._constants._public_network
                and self.broadcast_address in self._constants._public_network
            )
            and not self.is_private
        )


class _IPv4Constants:
    _linklocal_network = IPv4Network("169.254.0.0/16")
    _loopback_network = IPv4Network("127.0.0.0/8")
    _multicast_network = IPv4Network("224.0.0.0/4")
    _public_network = IPv4Network("100.64.0.0/10")
    _private_networks = [
        IPv4Network("0.0.0.0/8"),
        IPv4Network("10.0.0.0/8"),
        IPv4Network("127.0.0.0/8"),
        IPv4Network("169.254.0.0/16"),
        IPv4Network("172.16.0.0/12"),
        IPv4Network("192.0.0.0/29"),
        IPv4Network("192.0.0.170/31"),
        IPv4Network("192.0.2.0/24"),
        IPv4Network("192.168.0.0/16"),
        IPv4Network("198.18.0.0/15"),
        IPv4Network("198.51.100.0/24"),
        IPv4Network("203.0.113.0/24"),
        IPv4Network("240.0.0.0/4"),
        IPv4Network("255.255.255.255/32"),
    ]
    _reserved_network = IPv4Network("240.0.0.0/4")
    _unspecified_address = IPv4Address("0.0.0.0")


IPv4Address._constants = _IPv4Constants
IPv4Network._constants = _IPv4Constants


class _BaseV6:
    """Base IPv6 object."""

    __slots__ = ()
    _version = 6
    _ALL_ONES = (2**IPV6LENGTH) - 1
    _HEXTET_COUNT = 8
    _HEX_DIGITS = frozenset("0123456789ABCDEFabcdef")
    _max_prefixlen = IPV6LENGTH
    _netmask_cache = {}

    @classmethod
    def _make_netmask(cls, arg):
        """Make a (netmask, prefix_len) tuple from the given argument."""
        if arg not in cls._netmask_cache:
            if isinstance(arg, int):
                prefixlen = arg
                if not (0 <= prefixlen <= cls._max_prefixlen):
                    cls._report_invalid_netmask(prefixlen)
            else:
                prefixlen = cls._prefix_from_prefix_string(arg)
            netmask = IPv6Address(cls._ip_int_from_prefix(prefixlen))
            cls._netmask_cache[arg] = netmask, prefixlen
        return cls._netmask_cache[arg]

    @classmethod
    def _ip_int_from_string(cls, ip_str):
        """Turn an IPv6 ip_str into an integer."""
        if not ip_str:
            raise AddressValueError("Address cannot be empty")

        parts = ip_str.split(":")

        _min_parts = 3
        if len(parts) < _min_parts:
            msg = f"At least {_min_parts} parts expected in {ip_str!r}"
            raise AddressValueError(msg)

        if "." in parts[-1]:
            try:
                ipv4_int = IPv4Address(parts.pop())._ip
            except AddressValueError as exc:
                raise AddressValueError(f"{exc} in {ip_str!r}") from None
            parts.append(f"{(ipv4_int >> 16) & 0xFFFF:x}")
            parts.append(f"{ipv4_int & 0xFFFF:x}")

        _max_parts = cls._HEXTET_COUNT + 1
        if len(parts) > _max_parts:
            msg = f"At most {_max_parts - 1} colons permitted in {ip_str!r}"
            raise AddressValueError(msg)

        skip_index = None
        for i in range(1, len(parts) - 1):
            if not parts[i]:
                if skip_index is not None:
                    msg = f"At most one '::' permitted in {ip_str!r}"
                    raise AddressValueError(msg)
                skip_index = i

        if skip_index is not None:
            parts_hi = skip_index
            parts_lo = len(parts) - skip_index - 1
            if not parts[0]:
                parts_hi -= 1
                if parts_hi:
                    msg = f"Leading ':' only permitted as part of '::' in {ip_str!r}"
                    raise AddressValueError(msg)
            if not parts[-1]:
                parts_lo -= 1
                if parts_lo:
                    msg = f"Trailing ':' only permitted as part of '::' in {ip_str!r}"
                    raise AddressValueError(msg)
            parts_skipped = cls._HEXTET_COUNT - (parts_hi + parts_lo)
            if parts_skipped < 1:
                msg = f"Expected at most {cls._HEXTET_COUNT - 1} other parts with '::' in {ip_str!r}"
                raise AddressValueError(msg)
        else:
            if len(parts) != cls._HEXTET_COUNT:
                msg = f"Exactly {cls._HEXTET_COUNT} parts expected without '::' in {ip_str!r}"
                raise AddressValueError(msg)
            if not parts[0]:
                msg = f"Leading ':' only permitted as part of '::' in {ip_str!r}"
                raise AddressValueError(msg)
            if not parts[-1]:
                msg = f"Trailing ':' only permitted as part of '::' in {ip_str!r}"
                raise AddressValueError(msg)
            parts_hi = len(parts)
            parts_lo = 0
            parts_skipped = 0

        try:
            ip_int = 0
            for i in range(parts_hi):
                ip_int <<= 16
                ip_int |= cls._parse_hextet(parts[i])
            ip_int <<= 16 * parts_skipped
            for i in range(-parts_lo, 0):
                ip_int <<= 16
                ip_int |= cls._parse_hextet(parts[i])
            return ip_int
        except ValueError as exc:
            raise AddressValueError(f"{exc} in {ip_str!r}") from None

    @classmethod
    def _parse_hextet(cls, hextet_str):
        """Convert an IPv6 hextet string into an integer."""
        if not cls._HEX_DIGITS.issuperset(hextet_str):
            raise ValueError(f"Only hex digits permitted in {hextet_str!r}")
        if len(hextet_str) > 4:
            msg = f"At most 4 characters permitted in {hextet_str!r}"
            raise ValueError(msg)
        return int(hextet_str, 16)

    @classmethod
    def _compress_hextets(cls, hextets):
        """Compresses a list of hextets per RFC 5952."""
        best_doublecolon_start = -1
        best_doublecolon_len = 0
        doublecolon_start = -1
        doublecolon_len = 0
        for index, hextet in enumerate(hextets):
            if hextet == "0":
                doublecolon_len += 1
                if doublecolon_start == -1:
                    doublecolon_start = index
                if doublecolon_len > best_doublecolon_len:
                    best_doublecolon_len = doublecolon_len
                    best_doublecolon_start = doublecolon_start
            else:
                doublecolon_len = 0
                doublecolon_start = -1

        if best_doublecolon_len > 1:
            best_doublecolon_end = best_doublecolon_start + best_doublecolon_len
            if best_doublecolon_end == len(hextets):
                hextets += [""]
            hextets[best_doublecolon_start:best_doublecolon_end] = [""]
            if best_doublecolon_start == 0:
                hextets = [""] + hextets

        return hextets

    @classmethod
    def _string_from_ip_int(cls, ip_int=None):
        """Turns a 128-bit integer into hexadecimal notation."""
        if ip_int is None:
            ip_int = int(cls._ip)

        if ip_int > cls._ALL_ONES:
            raise ValueError("IPv6 address is too large")

        hex_str = f"{ip_int:032x}"
        hextets = [f"{int(hex_str[x : x + 4], 16):x}" for x in range(0, 32, 4)]
        hextets = cls._compress_hextets(hextets)
        return ":".join(hextets)

    def _explode_shorthand_ip_string(self):
        """Expand a shortened IPv6 address."""
        if isinstance(self, IPv6Network):
            ip_str = str(self.network_address)
        elif isinstance(self, IPv6Interface):
            ip_str = str(self.ip)
        else:
            ip_str = str(self)

        ip_str, scope_id = self._split_scope_id(ip_str)
        ip_int = self._ip_int_from_string(ip_str)
        hex_str = f"{ip_int:032x}"
        parts = [hex_str[x : x + 4] for x in range(0, 32, 4)]
        if isinstance(self, (_BaseNetwork, IPv6Interface)):
            return f"{':'.join(parts)}/{self._prefixlen}"
        if scope_id:
            return f"{':'.join(parts)}%{scope_id}"
        return ":".join(parts)

    def _reverse_pointer(self):
        """Return the reverse DNS pointer name for the IPv6 address."""
        reverse_chars = self.exploded[::-1].replace(":", "")
        return ".".join(reverse_chars) + ".ip6.arpa"

    @staticmethod
    def _split_scope_id(ip_str):
        """Helper function to parse IPv6 string address with scope id."""
        addr, sep, scope_id = ip_str.partition("%")
        if not sep:
            scope_id = None
        elif not scope_id or "%" in scope_id:
            raise AddressValueError(f'Invalid IPv6 address: "{ip_str}"')
        return addr, scope_id

    @property
    def max_prefixlen(self):
        return self._max_prefixlen

    @property
    def version(self):
        return self._version


class IPv6Address(_BaseV6, _BaseAddress):
    """Represent and manipulate single IPv6 Addresses."""

    __slots__ = ("_ip", "_scope_id", "__weakref__")

    def __init__(self, address):
        if isinstance(address, int):
            self._check_int_address(address)
            self._ip = address
            self._scope_id = None
            return

        if isinstance(address, bytes):
            self._check_packed_address(address, 16)
            self._ip = int.from_bytes(address, "big")
            self._scope_id = None
            return

        if isinstance(address, IPv6Address):
            self._ip = address._ip
            self._scope_id = address._scope_id
            return

        addr_str = str(address)
        if "/" in addr_str:
            raise AddressValueError(f"Unexpected '/' in {address!r}")
        addr_str, self._scope_id = self._split_scope_id(addr_str)
        self._ip = self._ip_int_from_string(addr_str)

    def __str__(self):
        ip_str = self._string_from_ip_int(self._ip)
        return ip_str + "%" + self._scope_id if self._scope_id else ip_str

    def __hash__(self):
        return hash((self._ip, self._scope_id))

    def __eq__(self, other):
        address_equal = super().__eq__(other)
        if address_equal is NotImplemented:
            return NotImplemented
        if not address_equal:
            return False
        return self._scope_id == getattr(other, "_scope_id", None)

    def __lt__(self, other):
        address_less = super().__lt__(other)
        if address_less is NotImplemented:
            return NotImplemented
        if self._ip != other._ip:
            return address_less
        other_scope = getattr(other, "_scope_id", None)
        if self._scope_id == other_scope:
            return False
        if self._scope_id is None:
            return True
        if other_scope is None:
            return False
        return self._scope_id < other_scope

    def __reduce__(self):
        if self._scope_id:
            return self.__class__, (str(self),)
        return self.__class__, (self._ip,)

    @property
    def scope_id(self):
        """Identifier of a particular zone of the address's scope."""
        return self._scope_id

    @property
    def packed(self):
        """The binary representation of this address."""
        return v6_int_to_packed(self._ip)

    @property
    def is_multicast(self):
        return self in self._constants._multicast_network

    @property
    def is_reserved(self):
        return any(self in x for x in self._constants._reserved_networks)

    @property
    def is_link_local(self):
        return self in self._constants._linklocal_network

    @property
    def is_site_local(self):
        return self in self._constants._sitelocal_network

    @property
    def is_private(self):
        return any(self in net for net in self._constants._private_networks)

    @property
    def is_global(self):
        return not self.is_private

    @property
    def is_unspecified(self):
        return self._ip == 0

    @property
    def is_loopback(self):
        return self._ip == 1

    @property
    def ipv4_mapped(self):
        if (self._ip >> 32) != 0xFFFF:
            return None
        return IPv4Address(self._ip & 0xFFFFFFFF)

    @property
    def teredo(self):
        if (self._ip >> 96) != 0x20010000:
            return None
        return (
            IPv4Address((self._ip >> 64) & 0xFFFFFFFF),
            IPv4Address(~self._ip & 0xFFFFFFFF),
        )

    @property
    def sixtofour(self):
        if (self._ip >> 112) != 0x2002:
            return None
        return IPv4Address((self._ip >> 80) & 0xFFFFFFFF)


class IPv6Interface(IPv6Address):
    def __init__(self, address):
        addr, mask = self._split_addr_prefix(address)
        IPv6Address.__init__(self, addr)
        self.network = IPv6Network((addr, mask), strict=False)
        self.netmask = self.network.netmask
        self._prefixlen = self.network._prefixlen

    @property
    def hostmask(self):
        return self.network.hostmask

    def __str__(self):
        return f"{self._string_from_ip_int(self._ip)}/{self._prefixlen}"

    def __eq__(self, other):
        address_equal = IPv6Address.__eq__(self, other)
        if address_equal is NotImplemented or not address_equal:
            return address_equal
        try:
            return self.network == other.network
        except AttributeError:
            return False

    def __lt__(self, other):
        address_less = IPv6Address.__lt__(self, other)
        if address_less is NotImplemented:
            return NotImplemented
        try:
            return self.network < other.network or (
                self.network == other.network and address_less
            )
        except AttributeError:
            return False

    def __hash__(self):
        return hash((self._ip, self._prefixlen, int(self.network.network_address)))

    __reduce__ = _IPAddressBase.__reduce__

    @property
    def ip(self):
        return IPv6Address(self._ip)

    @property
    def with_prefixlen(self):
        return f"{self._string_from_ip_int(self._ip)}/{self._prefixlen}"

    @property
    def with_netmask(self):
        return f"{self._string_from_ip_int(self._ip)}/{self.netmask}"

    @property
    def with_hostmask(self):
        return f"{self._string_from_ip_int(self._ip)}/{self.hostmask}"

    @property
    def is_unspecified(self):
        return self._ip == 0 and self.network.is_unspecified

    @property
    def is_loopback(self):
        return self._ip == 1 and self.network.is_loopback


class IPv6Network(_BaseV6, _BaseNetwork):
    """This class represents and manipulates 128-bit IPv6 networks."""

    _address_class = IPv6Address

    def __init__(self, address, strict=True):
        self._cache = {}
        addr, mask = self._split_addr_prefix(address)

        self.network_address = IPv6Address(addr)
        self.netmask, self._prefixlen = self._make_netmask(mask)
        packed = int(self.network_address)
        if packed & int(self.netmask) != packed:
            if strict:
                raise ValueError(f"{self} has host bits set")
            else:
                self.network_address = IPv6Address(packed & int(self.netmask))

        if self._prefixlen >= (self._max_prefixlen - 1):
            self.hosts = self.__iter__

    def hosts(self):
        """Generate Iterator over usable hosts in a network.

        For IPv6, the Subnet-Router anycast address (network_address) is
        excluded, while the last address is included (RFC 4291).
        """
        network = int(self.network_address)
        broadcast = int(self.broadcast_address)
        for x in range(network + 1, broadcast + 1):
            yield self._address_class(x)

    @property
    def is_site_local(self):
        return self.network_address.is_site_local and self.broadcast_address.is_site_local


class _IPv6Constants:
    _linklocal_network = IPv6Network("fe80::/10")
    _multicast_network = IPv6Network("ff00::/8")
    _private_networks = [
        IPv6Network("::1/128"),
        IPv6Network("::/128"),
        IPv6Network("::ffff:0:0/96"),
        IPv6Network("100::/64"),
        IPv6Network("2001::/23"),
        IPv6Network("2001:2::/48"),
        IPv6Network("2001:db8::/32"),
        IPv6Network("2001:10::/28"),
        IPv6Network("fc00::/7"),
        IPv6Network("fe80::/10"),
    ]
    _reserved_networks = [
        IPv6Network("::/8"),
        IPv6Network("100::/8"),
        IPv6Network("200::/7"),
        IPv6Network("400::/6"),
        IPv6Network("800::/5"),
        IPv6Network("1000::/4"),
        IPv6Network("4000::/3"),
        IPv6Network("6000::/3"),
        IPv6Network("8000::/3"),
        IPv6Network("a000::/3"),
        IPv6Network("c000::/3"),
        IPv6Network("e000::/4"),
        IPv6Network("f000::/5"),
        IPv6Network("f800::/6"),
        IPv6Network("fe00::/9"),
    ]
    _sitelocal_network = IPv6Network("fec0::/10")


IPv6Address._constants = _IPv6Constants
IPv6Network._constants = _IPv6Constants
