# CPython 3.12 parity tests for `uuid` (issue #3185).

import sys
import uuid
from uuid import (
    UUID,
    SafeUUID,
    RESERVED_NCS,
    RFC_4122,
    RESERVED_MICROSOFT,
    RESERVED_FUTURE,
    NAMESPACE_DNS,
    NAMESPACE_URL,
    NAMESPACE_OID,
    NAMESPACE_X500,
    uuid1,
    uuid3,
    uuid4,
    uuid5,
    getnode,
)

# 1. Module metadata & constants
print("mod_id:", uuid is sys.modules["uuid"])
print("has_all:", hasattr(uuid, "__all__"))
print("variants:", RESERVED_NCS, RFC_4122, RESERVED_MICROSOFT, RESERVED_FUTURE)
print("namespaces:", str(NAMESPACE_DNS), str(NAMESPACE_URL), str(NAMESPACE_OID), str(NAMESPACE_X500))
print("safe_enum:", repr(SafeUUID.safe), repr(SafeUUID.unsafe), repr(SafeUUID.unknown))
print("safe_vals:", SafeUUID.safe.value, SafeUUID.unsafe.value, SafeUUID.unknown.value)
print("safe_lookup:", SafeUUID(0) is SafeUUID.safe, SafeUUID(-1) is SafeUUID.unsafe, SafeUUID(None) is SafeUUID.unknown)

# 2. All 5 constructor forms produce identical UUIDs
u_hex = UUID("12345678-1234-5678-1234-567812345678")
u_braces = UUID("{12345678-1234-5678-1234-567812345678}")
u_plain = UUID("12345678123456781234567812345678")
u_upper = UUID("12345678-1234-5678-1234-567812345678".upper())
u_urn = UUID("urn:uuid:12345678-1234-5678-1234-567812345678")
u_bytes = UUID(bytes=b"\x12\x34\x56\x78" * 4)
u_bytes_le = UUID(bytes_le=b"\x78\x56\x34\x12\x34\x12\x78\x56\x12\x34\x56\x78\x12\x34\x56\x78")
u_fields = UUID(fields=(0x12345678, 0x1234, 0x5678, 0x12, 0x34, 0x567812345678))
u_int = UUID(int=0x12345678123456781234567812345678)

all_u = [u_hex, u_braces, u_plain, u_upper, u_urn, u_bytes, u_bytes_le, u_fields, u_int]
print("all_equal:", all(x == u_hex for x in all_u))
print("all_hash_equal:", len({hash(x) for x in all_u}) == 1)

# 3. Properties on a concrete UUID
u = UUID("a8098c1a-f86e-11da-bd1a-00112444be1e")
print("str:", str(u))
print("repr:", repr(u))
print("hex:", u.hex)
print("urn:", u.urn)
print("int:", hex(u.int), int(u) == u.int)
print("bytes:", u.bytes)
print("bytes_le:", u.bytes_le)
print("fields:", tuple(hex(f) for f in u.fields))
print(
    "field_attrs:",
    hex(u.time_low),
    hex(u.time_mid),
    hex(u.time_hi_version),
    hex(u.clock_seq_hi_variant),
    hex(u.clock_seq_low),
    hex(u.node),
)
print("time_clock_seq:", hex(u.time), hex(u.clock_seq))
print("variant_version:", u.variant, u.version)
print("is_safe:", u.is_safe, UUID(int=0, is_safe=SafeUUID.safe).is_safe)

# 4. Variant classification and version override
v_ncs = UUID(int=0)
v_rfc = UUID(int=0x8000 << 48)
v_ms = UUID(int=0xC000 << 48)
v_fut = UUID(int=0xE000 << 48)
print("variant_cases:", v_ncs.variant, v_ncs.version, v_rfc.variant, v_rfc.version, v_ms.variant, v_ms.version, v_fut.variant, v_fut.version)

for ver in range(1, 6):
    uv = UUID(int=0, version=ver)
    print("ver_override:", ver, uv.version, uv.variant, str(uv))

# 5. Ordering, equality, hashing, immutability, getstate/setstate
a = UUID(int=1)
b = UUID(int=2)
print("ordering:", a < b, a <= b, a <= a, b > a, b >= a, a != b, a == "not-a-uuid", a != "not-a-uuid")
try:
    _ = a < 1
except TypeError as e:
    print("order_type_err:", type(e).__name__)
try:
    a.int = 99
except TypeError as e:
    print("immut_err:", type(e).__name__, str(e))
try:
    a.foo = 1
except TypeError as e:
    print("immut_new_err:", type(e).__name__, str(e))

u_safe = UUID(int=42, is_safe=SafeUUID.safe)
st = u_safe.__getstate__()
print("getstate_safe:", st, UUID(int=42).__getstate__())
u_restored = object.__new__(UUID)
u_restored.__setstate__(st)
print("setstate_safe:", u_restored == u_safe, u_restored.is_safe is SafeUUID.safe)

# 6. Constructor validation errors
def check_err(label, fn):
    try:
        fn()
    except Exception as e:
        print(label, type(e).__name__, str(e))

check_err("no_args:", lambda: UUID())
check_err("two_args:", lambda: UUID(hex="0" * 32, int=0))
check_err("bad_hex_len:", lambda: UUID("1234"))
check_err("bad_hex_chars:", lambda: UUID("z" * 32))
check_err("bad_bytes_len:", lambda: UUID(bytes=b"short"))
check_err("bad_bytes_le_len:", lambda: UUID(bytes_le=b"short"))
check_err("bad_fields_len:", lambda: UUID(fields=(1, 2, 3)))
check_err("bad_field1:", lambda: UUID(fields=(1 << 32, 0, 0, 0, 0, 0)))
check_err("bad_field2:", lambda: UUID(fields=(0, 1 << 16, 0, 0, 0, 0)))
check_err("bad_field3:", lambda: UUID(fields=(0, 0, 1 << 16, 0, 0, 0)))
check_err("bad_field4:", lambda: UUID(fields=(0, 0, 0, 1 << 8, 0, 0)))
check_err("bad_field5:", lambda: UUID(fields=(0, 0, 0, 0, 1 << 8, 0)))
check_err("bad_field6:", lambda: UUID(fields=(0, 0, 0, 0, 0, 1 << 48)))
check_err("bad_int_neg:", lambda: UUID(int=-1))
check_err("bad_int_big:", lambda: UUID(int=1 << 128))
check_err("bad_ver_0:", lambda: UUID(int=0, version=0))
check_err("bad_ver_6:", lambda: UUID(int=0, version=6))

# 7. Deterministic uuid3 & uuid5 (both str and bytes names)
print("uuid3_dns:", str(uuid3(NAMESPACE_DNS, "python.org")))
print("uuid3_bytes:", str(uuid3(NAMESPACE_DNS, b"python.org")))
print("uuid3_url:", str(uuid3(NAMESPACE_URL, "https://python.org/")))
print("uuid3_oid:", str(uuid3(NAMESPACE_OID, "1.2.3.4")))
print("uuid3_x500:", str(uuid3(NAMESPACE_X500, "cn=test")))
print("uuid5_dns:", str(uuid5(NAMESPACE_DNS, "python.org")))
print("uuid5_bytes:", str(uuid5(NAMESPACE_DNS, b"python.org")))
print("uuid5_url:", str(uuid5(NAMESPACE_URL, "https://python.org/")))
print("uuid5_oid:", str(uuid5(NAMESPACE_OID, "1.2.3.4")))
print("uuid5_x500:", str(uuid5(NAMESPACE_X500, "cn=test")))
print("uuid3_meta:", uuid3(NAMESPACE_DNS, "python.org").version, uuid3(NAMESPACE_DNS, "python.org").variant)
print("uuid5_meta:", uuid5(NAMESPACE_DNS, "python.org").version, uuid5(NAMESPACE_DNS, "python.org").variant)

# 8. uuid4, uuid1, getnode properties
u4_a = uuid4()
u4_b = uuid4()
print("uuid4_props:", isinstance(u4_a, UUID), u4_a.version == 4, u4_a.variant == RFC_4122, u4_a != u4_b, len(str(u4_a)) == 36)

n1 = getnode()
n2 = getnode()
print("getnode_props:", isinstance(n1, int), 0 <= n1 < (1 << 48), n1 == n2)

u1_a = uuid1(node=0x112233445566, clock_seq=0x1234)
u1_b = uuid1(node=0x112233445566, clock_seq=0x1234)
print(
    "uuid1_props:",
    isinstance(u1_a, UUID),
    u1_a.version == 1,
    u1_a.variant == RFC_4122,
    u1_a.node == 0x112233445566,
    u1_a.clock_seq == 0x1234,
    u1_b.time > u1_a.time,
)
u1_def = uuid1()
print("uuid1_def:", u1_def.version == 1, u1_def.variant == RFC_4122, 0 <= u1_def.node < (1 << 48))
