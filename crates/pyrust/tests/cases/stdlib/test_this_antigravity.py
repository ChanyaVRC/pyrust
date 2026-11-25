# CPython 3.12 parity tests for the native `this` and `antigravity` modules (issue #3248).

import contextlib
import io
import sys
import types

# 1. `this` prints the decoded Zen of Python on first import
buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    import this

zen_output = buf.getvalue()
print("zen_len:", len(zen_output))
print("zen_first_line:", zen_output.splitlines()[0])
print("zen_last_line:", zen_output.splitlines()[-1])
print("this_c_i:", this.c, this.i)
print("this_d_len:", len(this.d))
print("this_d_sample:", this.d["A"], this.d["N"], this.d["a"], this.d["n"])
print("this_s_matches:", "".join(this.d.get(ch, ch) for ch in this.s) + "\n" == zen_output)

# Re-importing `this` from cache does not print again
buf2 = io.StringIO()
with contextlib.redirect_stdout(buf2):
    import this as this2

print("reimport_empty:", buf2.getvalue() == "", this2 is this)

# 2. `antigravity` — mock `webbrowser` before importing so neither CPython nor
# PyRust launches a real browser during headless test runs.
opened_urls = []
fake_wb = types.ModuleType("webbrowser")
fake_wb.open = lambda url: opened_urls.append(url)
sys.modules["webbrowser"] = fake_wb

import antigravity

print("opened_urls:", opened_urls)

# Test Munroe geohash algorithm (xkcd #426)
geo_buf = io.StringIO()
with contextlib.redirect_stdout(geo_buf):
    antigravity.geohash(37.421542, -122.085589, b"2005-05-26-10458.68")
    antigravity.geohash(0.0, 0.0, b"2024-01-01-37000.00")
    antigravity.geohash(-33.8688, 151.2093, bytearray(b"2005-05-26-10458.68"))
print("geohash_lines:", geo_buf.getvalue().splitlines())
