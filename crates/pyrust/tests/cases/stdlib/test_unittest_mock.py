# CPython 3.12 parity tests for `unittest.mock` (issue #3222).
import asyncio
import inspect
import sys
import types
import unittest

# Before importing unittest.mock, unittest does not eagerly expose `mock`.
print("has mock before import:", hasattr(unittest, "mock"))

import unittest.mock
from unittest import mock as mock_from_pkg
from unittest.mock import (
    ANY,
    DEFAULT,
    FILTER_DIR,
    AsyncMock,
    InvalidSpecError,
    MagicMock,
    Mock,
    NonCallableMagicMock,
    NonCallableMock,
    PropertyMock,
    call,
    create_autospec,
    mock_open,
    patch,
    seal,
    sentinel,
)

# ---------------------------------------------------------------------------
# 1. Module binding, identity, and public surface
# ---------------------------------------------------------------------------
print("has mock after import:", hasattr(unittest, "mock"))
print(
    "module identity:",
    unittest.mock is mock_from_pkg,
    unittest.mock is sys.modules["unittest.mock"],
    unittest.mock.__name__,
)
print("__all__:", unittest.mock.__all__)
print("FILTER_DIR:", FILTER_DIR)
print("InvalidSpecError base:", issubclass(InvalidSpecError, Exception))

# ---------------------------------------------------------------------------
# 2. Sentinels, ANY, DEFAULT, and call objects
# ---------------------------------------------------------------------------
print(
    "ANY eq:",
    ANY == 1,
    1 == ANY,
    ANY == None,
    None == ANY,
    (1, ANY) == (1, "anything"),
    {"a": ANY} == {"a": [1, 2, 3]},
    ANY != 1,
    repr(ANY),
)
print(
    "sentinel:",
    sentinel.foo is sentinel.foo,
    sentinel.foo == sentinel.foo,
    sentinel.foo != sentinel.bar,
    repr(sentinel.foo),
    repr(DEFAULT),
)

c1 = call(1, 2, a=3)
print("call basic:", c1 == call(1, 2, a=3), c1 != call(1, 2, a=4), c1.args, c1.kwargs)
chained = call.foo(1).bar(2, x=3)
print("call chained:", repr(chained), chained.call_list())
print("call with ANY:", call(1, ANY) == call(1, "hello"))

# ---------------------------------------------------------------------------
# 3. NonCallableMock & Mock basics, side_effect, wraps, configure_mock, reset_mock
# ---------------------------------------------------------------------------
ncm = NonCallableMock(name="ncm")
try:
    ncm()
    print("ncm callable: UNEXPECTED")
except TypeError:
    print("ncm not callable: OK")

m = Mock(return_value=42)
print("mock basic:", m.called, m.call_count, m.call_args)
res = m(1, 2, k="v")
print("mock after call:", res, m.called, m.call_count, m.call_args, m.call_args_list)
m.assert_called()
m.assert_called_once()
m.assert_called_with(1, 2, k="v")
m.assert_called_once_with(1, 2, k="v")
m.assert_any_call(1, 2, k="v")

# Assertion failures
m2 = Mock()
m2.assert_not_called()
for label, fn in [
    ("assert_called on uncalled", lambda: m2.assert_called()),
    ("assert_called_once on uncalled", lambda: m2.assert_called_once()),
    ("assert_called_with on uncalled", lambda: m2.assert_called_with(1)),
]:
    try:
        fn()
        print(label, "UNEXPECTED")
    except AssertionError:
        print(label, "AssertionError OK")

m2(1)
m2(2, b=3)
for label, fn in [
    ("assert_not_called on called", lambda: m2.assert_not_called()),
    ("assert_called_once on 2 calls", lambda: m2.assert_called_once()),
    ("assert_called_with mismatch", lambda: m2.assert_called_with(1)),
    ("assert_called_once_with on 2 calls", lambda: m2.assert_called_once_with(2, b=3)),
    ("assert_any_call missing", lambda: m2.assert_any_call(99)),
]:
    try:
        fn()
        print(label, "UNEXPECTED")
    except AssertionError:
        print(label, "AssertionError OK")

m2.assert_any_call(1)
m2.assert_called_with(2, b=3)
m2.assert_has_calls([call(1), call(2, b=3)])
m2.assert_has_calls([call(2, b=3), call(1)], any_order=True)
try:
    m2.assert_has_calls([call(2, b=3), call(1)], any_order=False)
    print("ordered fail: UNEXPECTED")
except AssertionError:
    print("ordered fail: AssertionError OK")

# side_effect: exception class, exception instance, iterable, callable, DEFAULT
m_exc_cls = Mock(side_effect=ValueError)
try:
    m_exc_cls()
except ValueError:
    print("side_effect exc class: OK")

m_exc_inst = Mock(side_effect=RuntimeError("boom"))
try:
    m_exc_inst()
except RuntimeError as e:
    print("side_effect exc inst:", str(e))

m_iter = Mock(side_effect=[10, 20, KeyError("k")])
print("side_effect iter 1:", m_iter())
print("side_effect iter 2:", m_iter())
try:
    m_iter()
except KeyError as e:
    print("side_effect iter 3:", e.args)
try:
    m_iter()
except StopIteration:
    print("side_effect iter exhausted: StopIteration OK")

m_fn = Mock(return_value="fallback")
m_fn.side_effect = lambda x: DEFAULT if x == 0 else x * 10
print("side_effect callable:", m_fn(3), m_fn(0))

# wraps
def real_add(a, b):
    return a + b


class RealCalc:
    factor = 3

    def mul(self, x):
        return x * self.factor


w_fn = Mock(wraps=real_add)
print("wraps fn:", w_fn(2, 5), w_fn.call_args)
w_fn.return_value = 999
print("wraps fn override:", w_fn(2, 5))

w_obj = Mock(wraps=RealCalc())
print("wraps obj:", w_obj.mul(4), w_obj.mul.call_args)

# attach_mock, method_calls, mock_calls, configure_mock, reset_mock
parent = Mock(name="parent")
child = Mock(name="child", return_value=7)
parent.attach_mock(child, "do_work")
print("attach_mock:", parent.do_work(1, k=2))
print("parent method_calls:", parent.method_calls)
print("parent mock_calls:", parent.mock_calls)

cfg = Mock()
cfg.configure_mock(return_value=5, **{"sub.deep.return_value": "ok", "x": 11})
print("configure_mock:", cfg(), cfg.sub.deep(), cfg.x)

rm = Mock(return_value=123, side_effect=[1, 2])
print("before reset:", rm(), rm.call_count)
rm.reset_mock()
print("after default reset:", rm.call_count, rm.called, rm.return_value, rm())
rm.reset_mock(return_value=True, side_effect=True)
print("after full reset:", rm.call_count, rm.side_effect, isinstance(rm.return_value, Mock))

# unsafe typo guard
safe_m = Mock()
try:
    _ = safe_m.assret_called()
    print("unsafe guard: UNEXPECTED")
except AttributeError:
    print("unsafe guard: AttributeError OK")

unsafe_m = Mock(unsafe=True)
unsafe_m.assret_called()
print("unsafe=True allowed: OK")

# ---------------------------------------------------------------------------
# 4. spec & spec_set
# ---------------------------------------------------------------------------
class SampleService:
    version = 1

    def fetch(self, url, timeout=5):
        return (url, timeout)

    async def afetch(self, url):
        return url


sp = Mock(spec=SampleService)
print(
    "spec isinstance:",
    isinstance(sp, SampleService),
    isinstance(sp, Mock),
    sp.__class__ is SampleService,
    type(sp) is not SampleService,
)
sp.fetch("https://example.com")
sp.fetch.assert_called_once_with("https://example.com")
print("spec async child is AsyncMock:", isinstance(sp.afetch, AsyncMock))
try:
    _ = sp.nonexistent
    print("spec unknown attr: UNEXPECTED")
except AttributeError:
    print("spec unknown attr: AttributeError OK")

# spec allows setting arbitrary attrs; spec_set forbids it
sp.extra_attr = 123
print("spec set extra:", sp.extra_attr)

ss = Mock(spec_set=SampleService)
ss.version = 2
print("spec_set valid set:", ss.version)
try:
    ss.not_on_spec = 99
    print("spec_set invalid set: UNEXPECTED")
except AttributeError:
    print("spec_set invalid set: AttributeError OK")

list_spec = Mock(spec=["alpha", "beta"])
list_spec.alpha()
try:
    _ = list_spec.gamma
    print("list spec invalid: UNEXPECTED")
except AttributeError:
    print("list spec invalid: AttributeError OK")

# ---------------------------------------------------------------------------
# 5. MagicMock & NonCallableMagicMock
# ---------------------------------------------------------------------------
ncmm = NonCallableMagicMock()
ncmm.__len__.return_value = 3
print("ncmm len:", len(ncmm))
try:
    ncmm()
    print("ncmm callable: UNEXPECTED")
except TypeError:
    print("ncmm not callable: OK")

mm = MagicMock()
mm.__len__.return_value = 4
mm.__iter__.return_value = iter([10, 20, 30])
mm.__getitem__.side_effect = lambda k: f"item:{k}"
mm.__contains__.side_effect = lambda k: k == "yes"
mm.__bool__.return_value = True
mm.__int__.return_value = 7
mm.__float__.return_value = 2.5
mm.__str__.return_value = "magic-str"
mm.__add__.return_value = 100
mm.__lt__.return_value = True
mm.__enter__.return_value = "entered"
mm.__exit__.return_value = False

print("mm len:", len(mm))
print("mm iter:", list(mm))
print("mm getitem:", mm["a"])
mm["k"] = "v"
del mm["k"]
mm.__setitem__.assert_called_once_with("k", "v")
mm.__delitem__.assert_called_once_with("k")
print("mm contains:", "yes" in mm, "no" in mm)
print("mm bool/int/float/str:", bool(mm), int(mm), float(mm), str(mm))
print("mm add/lt:", mm + 1, mm < 5)
with mm as ctx_val:
    print("mm ctx:", ctx_val)
mm.__enter__.assert_called_once()
mm.__exit__.assert_called_once_with(None, None, None)

# ---------------------------------------------------------------------------
# 6. AsyncMock
# ---------------------------------------------------------------------------
async def run_async_tests():
    am = AsyncMock(return_value="async-result")
    print(
        "AsyncMock iscoroutinefunction:",
        inspect.iscoroutinefunction(am),
        inspect.iscoroutinefunction(Mock()),
    )
    am.assert_not_awaited()
    v1 = await am(1, x=2)
    print("AsyncMock await 1:", v1, am.await_count, am.await_args, am.call_count)
    am.assert_awaited()
    am.assert_awaited_once()
    am.assert_awaited_with(1, x=2)
    am.assert_awaited_once_with(1, x=2)
    am.assert_any_await(1, x=2)

    v2 = await am(3)
    print("AsyncMock await 2:", v2, am.await_count, am.await_args_list)
    am.assert_has_awaits([call(1, x=2), call(3)])
    am.assert_has_awaits([call(3), call(1, x=2)], any_order=True)
    try:
        am.assert_awaited_once()
        print("assert_awaited_once fail: UNEXPECTED")
    except AssertionError:
        print("assert_awaited_once fail: AssertionError OK")
    try:
        am.assert_not_awaited()
        print("assert_not_awaited fail: UNEXPECTED")
    except AssertionError:
        print("assert_not_awaited fail: AssertionError OK")

    am.reset_mock()
    print("AsyncMock after reset:", am.await_count, am.await_args, am.await_args_list)
    am.assert_not_awaited()

    # AsyncMock side_effect: exception, iterable, sync callable, async callable
    am_exc = AsyncMock(side_effect=ValueError("async-boom"))
    try:
        await am_exc()
    except ValueError as e:
        print("AsyncMock exc:", str(e))

    am_iter = AsyncMock(side_effect=[100, 200])
    print("AsyncMock iter:", await am_iter(), await am_iter())

    async def double_async(n):
        return n * 2

    am_coro = AsyncMock(side_effect=double_async)
    print("AsyncMock async side_effect:", await am_coro(21))

    am_sync = AsyncMock(side_effect=lambda n: n + 1)
    print("AsyncMock sync side_effect:", await am_sync(41))

    # Async context manager & async iterator
    acm = AsyncMock()
    acm.__aenter__.return_value = "async-ctx"
    async with acm as val:
        print("AsyncMock aenter:", val)
    acm.__aenter__.assert_awaited_once()
    acm.__aexit__.assert_awaited_once_with(None, None, None)

    ait = MagicMock()
    ait.__aiter__.return_value = [1, 2, 3]
    collected = []
    async for item in ait:
        collected.append(item)
    print("AsyncMock aiter:", collected)


asyncio.run(run_async_tests())

# ---------------------------------------------------------------------------
# 7. PropertyMock
# ---------------------------------------------------------------------------
class HasProp:
    @property
    def prop(self):
        return "real"


hp = HasProp()
with patch.object(HasProp, "prop", new_callable=PropertyMock) as pm:
    pm.return_value = "mocked-prop"
    print("PropertyMock get:", hp.prop)
    hp.prop = "new-val"
    print("PropertyMock calls:", pm.mock_calls)
print("PropertyMock restored:", hp.prop)

# ---------------------------------------------------------------------------
# 8. patch, patch.object, patch.dict, patch.multiple, patch.stopall
# ---------------------------------------------------------------------------
# Create a synthetic module in sys.modules to test dotted string patching cleanly.
target_mod = types.ModuleType("_mock_patch_target_mod")
target_mod.VALUE = 10
target_mod.OTHER = 20
target_mod.CONFIG = {"a": 1, "b": 2}


def _orig_func(x):
    return x + 1


class _OrigClass:
    attr = "orig"

    def compute(self, x, y=1):
        return x + y


target_mod.func = _orig_func
target_mod.OrigClass = _OrigClass
sys.modules["_mock_patch_target_mod"] = target_mod

try:
    # Context manager
    with patch("_mock_patch_target_mod.VALUE", 99) as patched_val:
        print("patch cm:", patched_val, target_mod.VALUE)
    print("patch cm restored:", target_mod.VALUE)

    # create=True for nonexistent attribute
    with patch("_mock_patch_target_mod.NEW_ATTR", 555, create=True):
        print("patch create=True:", target_mod.NEW_ATTR)
    print("patch create=True cleaned:", hasattr(target_mod, "NEW_ATTR"))

    # Stacked @patch decorators — bottom-up argument order!
    @patch("_mock_patch_target_mod.VALUE", return_value="top")
    @patch("_mock_patch_target_mod.OTHER", return_value="bottom")
    def stacked_fn(mock_other, mock_value):
        return (mock_other(), mock_value(), target_mod.OTHER(), target_mod.VALUE())

    print("stacked @patch order:", stacked_fn())
    print("stacked restored:", target_mod.VALUE, target_mod.OTHER)

    # @patch on async function
    @patch("_mock_patch_target_mod.func", return_value=777)
    async def async_patched(m_func):
        return (target_mod.func(1), m_func.call_args)

    print("async @patch:", asyncio.run(async_patched()))
    print("async @patch restored:", target_mod.func(1))

    # @patch as class decorator
    @patch("_mock_patch_target_mod.VALUE", 321)
    class PatchedSuite:
        def test_one(self):
            return target_mod.VALUE

        def helper(self):
            return target_mod.VALUE

    ps = PatchedSuite()
    print("class @patch:", ps.test_one(), ps.helper(), target_mod.VALUE)

    # patch.object (cm + decorator + autospec)
    inst = _OrigClass()
    with patch.object(inst, "compute", autospec=True, return_value=42) as m_comp:
        print("patch.object autospec:", inst.compute(1, y=2))
        try:
            inst.compute(1, 2, 3)
            print("patch.object autospec bad sig: UNEXPECTED")
        except TypeError:
            print("patch.object autospec bad sig: TypeError OK")
    print("patch.object restored:", inst.compute(3, 4))

    # patch.dict (cm, clear=True, decorator, string target, exception rollback)
    d = {"x": 1, "y": 2}
    with patch.dict(d, {"y": 20, "z": 30}):
        print("patch.dict cm:", sorted(d.items()))
    print("patch.dict restored:", sorted(d.items()))

    with patch.dict(d, {"only": 9}, clear=True):
        print("patch.dict clear:", sorted(d.items()))
    print("patch.dict clear restored:", sorted(d.items()))

    with patch.dict("_mock_patch_target_mod.CONFIG", a=100, c=300):
        print("patch.dict str target:", sorted(target_mod.CONFIG.items()))
    print("patch.dict str restored:", sorted(target_mod.CONFIG.items()))

    try:
        with patch.dict(d, {"x": 999}):
            raise RuntimeError("abort")
    except RuntimeError:
        pass
    print("patch.dict exc restored:", sorted(d.items()))

    # patch.multiple (cm + decorator)
    with patch.multiple("_mock_patch_target_mod", VALUE=111, OTHER=DEFAULT) as mult:
        mult["OTHER"].return_value = 222
        print("patch.multiple cm:", target_mod.VALUE, target_mod.OTHER(), sorted(mult.keys()))
    print("patch.multiple restored:", target_mod.VALUE, target_mod.OTHER)

    @patch.multiple("_mock_patch_target_mod", VALUE=DEFAULT, OTHER=DEFAULT)
    def mult_deco(VALUE, OTHER):
        VALUE.return_value = "V"
        OTHER.return_value = "O"
        return (target_mod.VALUE(), target_mod.OTHER())

    print("patch.multiple deco:", mult_deco(), target_mod.VALUE, target_mod.OTHER)

    # Manual .start() / .stop() and patch.stopall()
    p1 = patch("_mock_patch_target_mod.VALUE", 1001)
    p2 = patch.object(target_mod, "OTHER", 2002)
    r1 = p1.start()
    r2 = p2.start()
    print("manual start:", r1, r2, target_mod.VALUE, target_mod.OTHER)
    p1.stop()
    print("after p1.stop:", target_mod.VALUE, target_mod.OTHER)
    patch.stopall()
    print("after stopall:", target_mod.VALUE, target_mod.OTHER)
finally:
    sys.modules.pop("_mock_patch_target_mod", None)

# ---------------------------------------------------------------------------
# 9. create_autospec
# ---------------------------------------------------------------------------
def sample_fn(a, b=2, *, c=3):
    return (a, b, c)


async def sample_async_fn(x, y=1):
    return x + y


auto_fn = create_autospec(sample_fn, return_value="auto")
print("autospec fn ok:", auto_fn(1, c=9))
auto_fn.assert_called_once_with(1, c=9)
try:
    auto_fn()
    print("autospec fn missing arg: UNEXPECTED")
except TypeError:
    print("autospec fn missing arg: TypeError OK")

auto_afn = create_autospec(sample_async_fn, return_value="auto-async")
print(
    "autospec async fn iscoro:",
    inspect.iscoroutinefunction(auto_afn),
    unittest.mock.iscoroutinefunction(auto_afn),
)
print("autospec async fn ok:", asyncio.run(auto_afn(5)))
try:
    asyncio.run(auto_afn(1, 2, 3))
    print("autospec async bad sig: UNEXPECTED")
except TypeError:
    print("autospec async bad sig: TypeError OK")

auto_cls = create_autospec(SampleService)
auto_inst = auto_cls()
auto_inst.fetch("https://x", timeout=10)
auto_inst.fetch.assert_called_once_with("https://x", timeout=10)
try:
    auto_inst.fetch()
    print("autospec method bad sig: UNEXPECTED")
except TypeError:
    print("autospec method bad sig: TypeError OK")
try:
    _ = auto_inst.missing_method
    print("autospec missing attr: UNEXPECTED")
except AttributeError:
    print("autospec missing attr: AttributeError OK")

try:
    create_autospec(Mock())
    print("autospec of Mock: UNEXPECTED")
except InvalidSpecError:
    print("autospec of Mock: InvalidSpecError OK")

# ---------------------------------------------------------------------------
# 10. mock_open
# ---------------------------------------------------------------------------
mo = mock_open(read_data="line1\nline2\nline3\n")
with mo("fake.txt", "r") as f:
    print("mock_open read:", repr(f.read()))
with mo("fake.txt", "r") as f:
    print("mock_open readline:", repr(f.readline()), repr(f.readline()))
with mo("fake.txt", "r") as f:
    print("mock_open readlines:", f.readlines())
with mo("fake.txt", "r") as f:
    print("mock_open iter:", list(f))
with mo("fake.txt", "w") as f:
    f.write("hello")
mo.return_value.write.assert_called_once_with("hello")

mo_bytes = mock_open(read_data=b"abc\ndef\n")
with mo_bytes("fake.bin", "rb") as fb:
    print("mock_open bytes:", fb.read())

# ---------------------------------------------------------------------------
# 11. seal
# ---------------------------------------------------------------------------
sealed = Mock()
sealed.existing.return_value = "alive"
sealed.sub.attr = 55
seal(sealed)
print("seal existing:", sealed.existing(), sealed.sub.attr)
for label, fn in [
    ("seal parent new attr", lambda: sealed.brand_new),
    ("seal child new attr", lambda: sealed.sub.other_new),
]:
    try:
        fn()
        print(label, "UNEXPECTED")
    except AttributeError:
        print(label, "AttributeError OK")

print("ALL unittest.mock PARITY TESTS PASSED")
