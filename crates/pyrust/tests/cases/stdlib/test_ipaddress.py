import ipaddress

# 1. Constants and exception hierarchy
assert ipaddress.IPV4LENGTH == 32
assert ipaddress.IPV6LENGTH == 128
assert issubclass(ipaddress.AddressValueError, ValueError)
assert issubclass(ipaddress.NetmaskValueError, ValueError)
print("constants_ok")

# 2. IPv4Address construction, properties, formatting, arithmetic, ordering, hashing
a_str = ipaddress.IPv4Address("192.168.1.10")
a_int = ipaddress.IPv4Address(0xC0A8010A)
a_bytes = ipaddress.IPv4Address(b"\xc0\xa8\x01\x0a")
a_copy = ipaddress.IPv4Address(a_str)
assert a_str == a_int == a_bytes == a_copy
assert hash(a_str) == hash(a_int) == hash(a_bytes)
assert a_str.version == 4
assert a_str.max_prefixlen == 32
assert a_str.packed == b"\xc0\xa8\x01\x0a"
assert a_str.compressed == "192.168.1.10"
assert a_str.exploded == "192.168.1.10"
assert a_str.reverse_pointer == "10.1.168.192.in-addr.arpa"
assert int(a_str) == 3232235786
assert str(a_str) == "192.168.1.10"
assert repr(a_str) == "IPv4Address('192.168.1.10')"
assert format(a_str, "s") == "192.168.1.10"
assert format(a_str, "b") == "11000000101010000000000100001010"
assert format(a_str, "#x") == "0xc0a8010a"
assert format(a_str, "X") == "C0A8010A"
assert format(a_str, "n") == "11000000101010000000000100001010"
assert format(a_str, "_X") == "C0A8_010A"
assert a_str + 5 == ipaddress.IPv4Address("192.168.1.15")
assert a_str - 10 == ipaddress.IPv4Address("192.168.1.0")
assert ipaddress.IPv4Address("10.0.0.1") < ipaddress.IPv4Address("10.0.0.2")
print("ipv4_addr_ok", a_str, a_str.reverse_pointer)

# IPv4 classification properties
v4_checks = [
    ("0.0.0.0", "unspecified"),
    ("127.0.0.1", "loopback"),
    ("10.1.2.3", "private"),
    ("172.16.5.1", "private"),
    ("192.168.0.1", "private"),
    ("169.254.1.2", "link_local"),
    ("224.0.0.1", "multicast"),
    ("240.0.0.1", "reserved"),
    ("8.8.8.8", "global"),
    ("100.64.0.1", "cgnat"),
]
for s, label in v4_checks:
    ip = ipaddress.IPv4Address(s)
    print(
        label,
        ip.is_multicast,
        ip.is_private,
        ip.is_global,
        ip.is_unspecified,
        ip.is_reserved,
        ip.is_loopback,
        ip.is_link_local,
    )

# IPv4Address validation errors
for bad in ["", "1.2.3", "1.2.3.4.5", "256.0.0.1", "01.2.3.4", "1.2.3.4/24", -1, 2**32, b"\x01\x02\x03"]:
    try:
        ipaddress.IPv4Address(bad)
        assert False, f"expected error for {bad!r}"
    except ipaddress.AddressValueError:
        pass
assert ipaddress.IPv4Address(True) == ipaddress.IPv4Address("0.0.0.1")
print("ipv4_errors_ok")

# 3. IPv6Address construction, compression, explosion, embedded IPv4, special attrs
v6_lo = ipaddress.IPv6Address("::1")
v6_full = ipaddress.IPv6Address("0000:0000:0000:0000:0000:0000:0000:0001")
v6_int = ipaddress.IPv6Address(1)
v6_bytes = ipaddress.IPv6Address(b"\x00" * 15 + b"\x01")
assert v6_lo == v6_full == v6_int == v6_bytes
assert hash(v6_lo) == hash(v6_full) == hash(v6_int)
assert v6_lo.version == 6
assert v6_lo.compressed == "::1"
assert v6_lo.exploded == "0000:0000:0000:0000:0000:0000:0000:0001"
assert v6_lo.packed == b"\x00" * 15 + b"\x01"
assert v6_lo.reverse_pointer == (
    "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.ip6.arpa"
)
assert v6_lo + 1 == ipaddress.IPv6Address("::2")
assert v6_lo - 1 == ipaddress.IPv6Address("::")
assert format(ipaddress.IPv6Address("2001:db8::1"), "X") == "20010DB8000000000000000000000001"
assert format(ipaddress.IPv6Address("2001:db8::1"), "n") == "20010db8000000000000000000000001"

# RFC 5952 compression rules: single 0 not collapsed; longest run wins; first tie wins
assert str(ipaddress.IPv6Address("2001:db8:0:1:1:1:1:1")) == "2001:db8:0:1:1:1:1:1"
assert str(ipaddress.IPv6Address("2001:0:0:1:0:0:0:1")) == "2001:0:0:1::1"
assert str(ipaddress.IPv6Address("2001:db8:0:0:1:0:0:1")) == "2001:db8::1:0:0:1"

# Scope ID
scoped = ipaddress.IPv6Address("fe80::1%eth0")
assert scoped.scope_id == "eth0"
assert str(scoped) == "fe80::1%eth0"
assert scoped != ipaddress.IPv6Address("fe80::1%eth1")
assert scoped != ipaddress.IPv6Address("fe80::1")

# Embedded IPv4, ipv4_mapped, sixtofour, teredo
mapped = ipaddress.IPv6Address("::ffff:192.0.2.128")
assert mapped.ipv4_mapped == ipaddress.IPv4Address("192.0.2.128")
assert v6_lo.ipv4_mapped is None

stf = ipaddress.IPv6Address("2002:c000:0201::1")
assert stf.sixtofour == ipaddress.IPv4Address("192.0.2.1")
assert v6_lo.sixtofour is None

teredo = ipaddress.IPv6Address("2001:0000:4136:e378:8000:63bf:3fff:fdd2")
assert teredo.teredo == (
    ipaddress.IPv4Address("65.54.227.120"),
    ipaddress.IPv4Address("192.0.2.45"),
)
assert v6_lo.teredo is None

# IPv6 classification properties
for s in ["::", "::1", "fc00::1", "fe80::1", "ff02::1", "2001:db8::1", "2606:4700:4700::1111"]:
    ip = ipaddress.IPv6Address(s)
    print(
        s,
        ip.is_multicast,
        ip.is_private,
        ip.is_global,
        ip.is_unspecified,
        ip.is_reserved,
        ip.is_loopback,
        ip.is_link_local,
    )

# Cross-version comparisons
assert (ipaddress.IPv4Address("127.0.0.1") == ipaddress.IPv6Address("::1")) is False
assert (ipaddress.IPv4Address("127.0.0.1") != ipaddress.IPv6Address("::1")) is True
try:
    _ = ipaddress.IPv4Address("127.0.0.1") < ipaddress.IPv6Address("::1")
    assert False
except TypeError:
    pass
print("ipv6_addr_ok")

# 4. IPv4Network & IPv6Network
n4 = ipaddress.IPv4Network("192.0.2.0/24")
assert n4.version == 4
assert n4.network_address == ipaddress.IPv4Address("192.0.2.0")
assert n4.broadcast_address == ipaddress.IPv4Address("192.0.2.255")
assert n4.netmask == ipaddress.IPv4Address("255.255.255.0")
assert n4.hostmask == ipaddress.IPv4Address("0.0.0.255")
assert n4.prefixlen == 24
assert n4.num_addresses == 256
assert n4.with_prefixlen == "192.0.2.0/24"
assert n4.with_netmask == "192.0.2.0/255.255.255.0"
assert n4.with_hostmask == "192.0.2.0/0.0.0.255"
assert ipaddress.IPv4Address("192.0.2.42") in n4
assert ipaddress.IPv4Address("192.0.3.1") not in n4
assert n4[0] == ipaddress.IPv4Address("192.0.2.0")
assert n4[1] == ipaddress.IPv4Address("192.0.2.1")
assert n4[-1] == ipaddress.IPv4Address("192.0.2.255")
assert n4[-2] == ipaddress.IPv4Address("192.0.2.254")
assert ipaddress.IPv4Network("192.0.2.0/255.255.255.0") == n4
assert ipaddress.IPv4Network("192.0.2.0/0.0.0.255") == n4
assert ipaddress.IPv4Network(("192.0.2.0", 24)) == n4
assert ipaddress.IPv4Network("192.0.2.77/24", strict=False) == n4

try:
    ipaddress.IPv4Network("192.0.2.77/24", strict=True)
    assert False
except ValueError:
    pass

# hosts(), subnets(), supernet(), overlaps(), subnet_of(), supernet_of(), address_exclude()
small4 = ipaddress.IPv4Network("192.0.2.0/30")
assert list(small4) == [
    ipaddress.IPv4Address("192.0.2.0"),
    ipaddress.IPv4Address("192.0.2.1"),
    ipaddress.IPv4Address("192.0.2.2"),
    ipaddress.IPv4Address("192.0.2.3"),
]
assert list(small4.hosts()) == [
    ipaddress.IPv4Address("192.0.2.1"),
    ipaddress.IPv4Address("192.0.2.2"),
]
assert list(ipaddress.IPv4Network("192.0.2.0/31").hosts()) == [
    ipaddress.IPv4Address("192.0.2.0"),
    ipaddress.IPv4Address("192.0.2.1"),
]
assert list(ipaddress.IPv4Network("192.0.2.1/32").hosts()) == [
    ipaddress.IPv4Address("192.0.2.1"),
]
subs = list(n4.subnets(prefixlen_diff=2))
assert len(subs) == 4
assert [str(s) for s in subs] == [
    "192.0.2.0/26",
    "192.0.2.64/26",
    "192.0.2.128/26",
    "192.0.2.192/26",
]
assert list(n4.subnets(new_prefix=25)) == [
    ipaddress.IPv4Network("192.0.2.0/25"),
    ipaddress.IPv4Network("192.0.2.128/25"),
]
assert n4.supernet() == ipaddress.IPv4Network("192.0.2.0/23")
assert n4.supernet(new_prefix=16) == ipaddress.IPv4Network("192.0.0.0/16")
assert subs[0].subnet_of(n4) is True
assert n4.supernet_of(subs[0]) is True
assert n4.subnet_of(subs[0]) is False
assert n4.overlaps(subs[0]) is True
assert subs[0].overlaps(subs[1]) is False
excluded = list(n4.address_exclude(ipaddress.IPv4Network("192.0.2.0/26")))
assert [str(x) for x in excluded] == [
    "192.0.2.128/25",
    "192.0.2.64/26",
]

n6 = ipaddress.IPv6Network("2001:db8::/126")
assert n6.num_addresses == 4
assert list(n6.hosts()) == [
    ipaddress.IPv6Address("2001:db8::1"),
    ipaddress.IPv6Address("2001:db8::2"),
    ipaddress.IPv6Address("2001:db8::3"),
]
assert list(ipaddress.IPv6Network("2001:db8::/127").hosts()) == [
    ipaddress.IPv6Address("2001:db8::"),
    ipaddress.IPv6Address("2001:db8::1"),
]
assert list(ipaddress.IPv6Network("2001:db8::1/128").hosts()) == [
    ipaddress.IPv6Address("2001:db8::1"),
]
assert n6.supernet(new_prefix=124) == ipaddress.IPv6Network("2001:db8::/124")
print("networks_ok", [str(x) for x in excluded])

# 5. IPv4Interface & IPv6Interface
if4 = ipaddress.IPv4Interface("192.0.2.10/24")
assert if4.ip == ipaddress.IPv4Address("192.0.2.10")
assert if4.network == ipaddress.IPv4Network("192.0.2.0/24")
assert if4.with_prefixlen == "192.0.2.10/24"
assert if4.with_netmask == "192.0.2.10/255.255.255.0"
assert if4.with_hostmask == "192.0.2.10/0.0.0.255"
assert str(if4) == "192.0.2.10/24"
assert repr(if4) == "IPv4Interface('192.0.2.10/24')"
assert if4 != ipaddress.IPv4Interface("192.0.2.10/16")
assert if4 != ipaddress.IPv4Address("192.0.2.10")

if6 = ipaddress.IPv6Interface("2001:db8::5/64")
assert if6.ip == ipaddress.IPv6Address("2001:db8::5")
assert if6.network == ipaddress.IPv6Network("2001:db8::/64")
assert if6.with_prefixlen == "2001:db8::5/64"
assert if6.with_netmask == "2001:db8::5/ffff:ffff:ffff:ffff::"
assert if6.with_hostmask == "2001:db8::5/::ffff:ffff:ffff:ffff"
print("interfaces_ok", if4, if6)

# 6. Factory and helper functions
assert isinstance(ipaddress.ip_address("10.0.0.1"), ipaddress.IPv4Address)
assert isinstance(ipaddress.ip_address("2001:db8::1"), ipaddress.IPv6Address)
assert isinstance(ipaddress.ip_network("10.0.0.0/8"), ipaddress.IPv4Network)
assert isinstance(ipaddress.ip_network("2001:db8::/32"), ipaddress.IPv6Network)
assert isinstance(ipaddress.ip_interface("10.0.0.1/8"), ipaddress.IPv4Interface)
assert isinstance(ipaddress.ip_interface("2001:db8::1/32"), ipaddress.IPv6Interface)

assert ipaddress.v4_int_to_packed(0x0A000001) == b"\x0a\x00\x00\x01"
assert ipaddress.v6_int_to_packed(1) == b"\x00" * 15 + b"\x01"

rng = list(
    ipaddress.summarize_address_range(
        ipaddress.IPv4Address("192.0.2.0"),
        ipaddress.IPv4Address("192.0.2.130"),
    )
)
print("summarize:", [str(n) for n in rng])

collapsed = list(
    ipaddress.collapse_addresses(
        [
            ipaddress.IPv4Network("192.0.2.0/25"),
            ipaddress.IPv4Network("192.0.2.128/25"),
            ipaddress.IPv4Address("192.0.3.1"),
            ipaddress.IPv4Address("192.0.3.0"),
        ]
    )
)
print("collapsed:", [str(n) for n in collapsed])

mixed = [
    ipaddress.IPv6Address("::1"),
    ipaddress.IPv4Address("127.0.0.1"),
    ipaddress.IPv4Address("10.0.0.1"),
    ipaddress.IPv6Address("2001:db8::1"),
]
mixed_sorted = sorted(mixed, key=ipaddress.get_mixed_type_key)
print("mixed_sorted:", [str(x) for x in mixed_sorted])

mixed_nets = [
    ipaddress.IPv6Network("2001:db8::/32"),
    ipaddress.IPv4Network("192.0.2.0/24"),
    ipaddress.IPv4Network("10.0.0.0/8"),
]
mixed_nets_sorted = sorted(mixed_nets, key=ipaddress.get_mixed_type_key)
print("mixed_nets_sorted:", [str(x) for x in mixed_nets_sorted])
