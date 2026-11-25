import datetime
import subprocess
import sys
import urllib.request as ureq
import xmlrpc
import xmlrpc.client as xc
import xmlrpc.server as xs

# ---------------------------------------------------------------------------
# 1. Module identity & constants & exception hierarchy
# ---------------------------------------------------------------------------
assert xmlrpc.client is sys.modules["xmlrpc.client"]
assert xmlrpc.server is sys.modules["xmlrpc.server"]
assert xc.PARSE_ERROR == -32700
assert xc.METHOD_NOT_FOUND == -32601
assert xc.MAXINT == 2147483647
assert xc.MININT == -2147483648
assert issubclass(xc.ProtocolError, xc.Error)
assert issubclass(xc.ResponseError, xc.Error)
assert issubclass(xc.Fault, xc.Error)
assert issubclass(xc.Error, Exception)
print("xmlrpc_imports_ok")

# ---------------------------------------------------------------------------
# 2. DateTime & Binary types
# ---------------------------------------------------------------------------
dt1 = xc.DateTime("20260501T12:30:45")
dt2 = xc.DateTime("20260501T12:30:46")
assert str(dt1) == "20260501T12:30:45"
assert dt1 < dt2
assert dt1 == "20260501T12:30:45"
assert dt1 == datetime.datetime(2026, 5, 1, 12, 30, 45)
tt = dt1.timetuple()
assert (tt.tm_year, tt.tm_mon, tt.tm_mday, tt.tm_hour, tt.tm_min, tt.tm_sec) == (
    2026,
    5,
    1,
    12,
    30,
    45,
)

b1 = xc.Binary(b"hello\x00world")
assert b1.data == b"hello\x00world"
assert b1 == b"hello\x00world"
assert b1 == xc.Binary(b"hello\x00world")
b2 = xc.Binary()
b2.decode(b"aGVsbG8=")
assert b2.data == b"hello"
print("xmlrpc_types_ok")

# ---------------------------------------------------------------------------
# 3. dumps / loads round-trip across all XML-RPC wire types
# ---------------------------------------------------------------------------
sample_params = (
    42,
    -7,
    True,
    False,
    3.5,
    "hello <world> & 'friends'",
    None,
    [1, "two", [3, False]],
    {"k1": "v1", "nested": {"x": 99}},
    xc.DateTime("20260501T12:30:45"),
    xc.Binary(b"\x01\x02\x03xyz"),
)
wire_call = xc.dumps(sample_params, methodname="demo.echo", allow_none=True)
assert "<methodCall>" in wire_call
assert "<methodName>demo.echo</methodName>" in wire_call
assert "<nil/>" in wire_call
assert "<base64>" in wire_call
assert "<dateTime.iso8601>20260501T12:30:45</dateTime.iso8601>" in wire_call

parsed_params, parsed_method = xc.loads(wire_call)
assert parsed_method == "demo.echo"
assert parsed_params[0] == 42
assert parsed_params[1] == -7
assert parsed_params[2] is True
assert parsed_params[3] is False
assert parsed_params[4] == 3.5
assert parsed_params[5] == "hello <world> & 'friends'"
assert parsed_params[6] is None
assert parsed_params[7] == [1, "two", [3, False]]
assert parsed_params[8] == {"k1": "v1", "nested": {"x": 99}}
assert isinstance(parsed_params[9], xc.DateTime)
assert parsed_params[9].value == "20260501T12:30:45"
assert isinstance(parsed_params[10], xc.Binary)
assert parsed_params[10].data == b"\x01\x02\x03xyz"

# use_builtin_types=True yields datetime.datetime and bytes
parsed_builtin, _ = xc.loads(wire_call, use_builtin_types=True)
assert isinstance(parsed_builtin[9], datetime.datetime)
assert parsed_builtin[9] == datetime.datetime(2026, 5, 1, 12, 30, 45)
assert isinstance(parsed_builtin[10], bytes)
assert parsed_builtin[10] == b"\x01\x02\x03xyz"

# <i8> & bare <value>text</value> decoding
i8_xml = (
    "<?xml version='1.0'?>"
    "<methodResponse><params>"
    "<param><value><i8>10000000000</i8></value></param>"
    "<param><value>bare string</value></param>"
    "</params></methodResponse>"
)
vals_i8, m_i8 = xc.loads(i8_xml)
assert m_i8 is None
assert vals_i8 == (10000000000, "bare string")

# None rejected unless allow_none=True; int overflow raises OverflowError
try:
    xc.dumps((None,))
    assert False, "expected TypeError for None without allow_none"
except TypeError:
    pass

try:
    xc.dumps((10**12,))
    assert False, "expected OverflowError for >32-bit int"
except OverflowError:
    pass

# Fault round-trip
fault_xml = xc.dumps(xc.Fault(404, "not found"))
try:
    xc.loads(fault_xml)
    assert False, "expected Fault"
except xc.Fault as f:
    assert f.faultCode == 404
    assert f.faultString == "not found"
    assert repr(f) == "<Fault 404: 'not found'>"
print("xmlrpc_codec_ok")

# ---------------------------------------------------------------------------
# 4. SimpleXMLRPCDispatcher._marshaled_dispatch, introspection & multicall
# ---------------------------------------------------------------------------
disp = xs.SimpleXMLRPCDispatcher(allow_none=True)


def add(a, b):
    """Add two integers."""
    return a + b


disp.register_function(add)


@disp.register_function(name="math.mul")
def multiply(a, b):
    """Multiply two numbers."""
    return a * b


class Service:
    def greet(self, who):
        """Greet someone."""
        return f"hi {who}"

    def _hidden(self):
        return "secret"


disp.register_instance(Service())
disp.register_introspection_functions()
disp.register_multicall_functions()

# Dispatch add
req_add = xc.dumps((19, 23), "add").encode("utf-8")
resp_add = disp._marshaled_dispatch(req_add)
assert xc.loads(resp_add) == ((42,), None)

# Dispatch dotted registered function
req_mul = xc.dumps((6, 7), "math.mul").encode("utf-8")
assert xc.loads(disp._marshaled_dispatch(req_mul)) == ((42,), None)

# Dispatch instance method
req_greet = xc.dumps(("pyrust",), "greet").encode("utf-8")
assert xc.loads(disp._marshaled_dispatch(req_greet)) == (("hi pyrust",), None)

# Private instance method rejected as Fault
req_priv = xc.dumps((), "_hidden").encode("utf-8")
try:
    xc.loads(disp._marshaled_dispatch(req_priv))
    assert False, "expected Fault for private method"
except xc.Fault as f:
    assert f.faultCode == 1

# Introspection
methods_xml = disp._marshaled_dispatch(xc.dumps((), "system.listMethods").encode("utf-8"))
(methods_list,), _ = xc.loads(methods_xml)
assert "add" in methods_list
assert "math.mul" in methods_list
assert "greet" in methods_list
assert "_hidden" not in methods_list
assert "system.multicall" in methods_list

help_xml = disp._marshaled_dispatch(xc.dumps(("add",), "system.methodHelp").encode("utf-8"))
assert xc.loads(help_xml) == (("Add two integers.",), None)

sig_xml = disp._marshaled_dispatch(
    xc.dumps(("add",), "system.methodSignature").encode("utf-8")
)
assert xc.loads(sig_xml) == (("signatures not supported",), None)

# Multicall
mc_req = xc.dumps(
    (
        [
            {"methodName": "add", "params": [2, 3]},
            {"methodName": "math.mul", "params": [4, 5]},
            {"methodName": "nonexistent", "params": []},
        ],
    ),
    "system.multicall",
).encode("utf-8")
(mc_res,), _ = xc.loads(disp._marshaled_dispatch(mc_req))
assert mc_res[0] == [5]
assert mc_res[1] == [20]
assert isinstance(mc_res[2], dict) and mc_res[2]["faultCode"] == 1
print("xmlrpc_dispatcher_ok")

# ---------------------------------------------------------------------------
# 5. Loopback ServerProxy <-> SimpleXMLRPCServer & DocXMLRPCServer (subprocess)
# ---------------------------------------------------------------------------
_XMLRPC_SERVER_CODE = '''
import sys
import xmlrpc.client as xc
import xmlrpc.server as xs

with xs.DocXMLRPCServer(("127.0.0.1", 0), logRequests=False, allow_none=True) as srv:
    srv.set_server_title("PyRust Test RPC")
    srv.set_server_name("PyRust RPC")
    srv.register_introspection_functions()
    srv.register_multicall_functions()

    def add(x, y):
        """Return x + y."""
        return x + y

    def echo_none(v):
        return v

    def boom():
        raise xc.Fault(99, "kaboom")

    srv.register_function(add)
    srv.register_function(echo_none)
    srv.register_function(boom)

    port = srv.server_address[1]
    sys.stdout.write(f"READY {port}\\n")
    sys.stdout.flush()
    for _ in range(6):
        srv.handle_request()
'''

proc = subprocess.Popen(
    [sys.executable, "-c", _XMLRPC_SERVER_CODE],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
try:
    ready = proc.stdout.readline().decode("ascii").strip()
    assert ready.startswith("READY "), ready
    port = int(ready.split()[1])
    url = f"http://127.0.0.1:{port}/RPC2"

    with xc.ServerProxy(url, allow_none=True) as proxy:
        # 1. Normal call
        assert proxy.add(10, 32) == 42
        # 2. None round-trip with allow_none=True
        assert proxy.echo_none(None) is None
        # 3. Introspection call
        ms = proxy.system.listMethods()
        assert "add" in ms and "boom" in ms
        # 4. Fault propagation
        try:
            proxy.boom()
            assert False, "expected Fault"
        except xc.Fault as f:
            assert f.faultCode == 99 and f.faultString == "kaboom"
        # 5. MultiCall client
        mc = xc.MultiCall(proxy)
        mc.add(1, 2)
        mc.add(3, 4)
        results = list(mc())
        assert results == [3, 7]

    # 6. DocXMLRPCServer GET documentation page
    with ureq.urlopen(f"http://127.0.0.1:{port}/", timeout=5) as resp:
        assert resp.status == 200
        doc_html = resp.read().decode("utf-8")
        assert "PyRust Test RPC" in doc_html
        assert "add" in doc_html
finally:
    if proc.poll() is None:
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
            proc.wait()
assert proc.returncode == 0
print("xmlrpc_loopback_ok")
print("xmlrpc_all_ok")
