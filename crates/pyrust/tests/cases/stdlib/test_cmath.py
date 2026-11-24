# CPython 3.12 parity tests for `cmath` (issue #3185).

import sys
import math
import cmath


def sign_tag(x):
    if math.isnan(x):
        return "nan"
    if math.isinf(x):
        return "-inf" if x < 0 else "+inf"
    if x == 0.0:
        return "-0.0" if math.copysign(1.0, x) < 0 else "+0.0"
    return f"{x:.10g}"


def fmt_c(z):
    return f"({sign_tag(z.real)}, {sign_tag(z.imag)})"


def fmt_special(z):
    def tok(x):
        if math.isnan(x):
            return "nan"
        if math.isinf(x):
            return "-inf" if x < 0 else "+inf"
        if x == 0.0:
            return "-0.0" if math.copysign(1.0, x) < 0 else "+0.0"
        # Check exact multiples of pi/4 or ±1
        ratio = x / (math.pi / 4)
        if abs(ratio - round(ratio)) < 1e-12 and abs(round(ratio)) <= 4:
            return f"{round(ratio)}*pi/4"
        if abs(x - 1.0) < 1e-12:
            return "+1"
        if abs(x + 1.0) < 1e-12:
            return "-1"
        return f"{x:.9g}"
    return f"({tok(z.real)}, {tok(z.imag)})"


# 1. Module identity & constants
print("mod_id:", cmath is sys.modules["cmath"])
print("pi_e_tau:", cmath.pi == math.pi, cmath.e == math.e, cmath.tau == math.tau)
print("inf_infj:", math.isinf(cmath.inf), cmath.inf > 0, cmath.infj.real == 0.0, math.isinf(cmath.infj.imag), cmath.infj.imag > 0)
print("nan_nanj:", math.isnan(cmath.nan), cmath.nanj.real == 0.0, math.isnan(cmath.nanj.imag))

# 2. Classification predicates
for z in [0, 1 + 2j, cmath.inf, cmath.infj, -cmath.inf + 1j, cmath.nan, cmath.nanj, complex(cmath.inf, cmath.nan)]:
    print("preds:", cmath.isfinite(z), cmath.isinf(z), cmath.isnan(z))

# 3. phase, polar, rect
for z in [1 + 0j, 0 + 1j, -1 + 0j, complex(-1.0, -0.0), 0 - 1j, 3 + 4j, -3 - 4j]:
    r, phi = cmath.polar(z)
    p = cmath.phase(z)
    rt = cmath.rect(r, phi)
    print("polar_rect:", round(r, 10), round(phi, 10), p == phi, cmath.isclose(rt, z, rel_tol=1e-12, abs_tol=1e-12))

# rect special values
for r in [0.0, -0.0, 1.0, -1.0, cmath.inf, -cmath.inf, cmath.nan]:
    for phi in [0.0, -0.0, 1.0, cmath.inf, -cmath.inf, cmath.nan]:
        try:
            res = cmath.rect(r, phi)
            print("rect_sp:", sign_tag(r), sign_tag(phi), "->", fmt_special(res))
        except Exception as e:
            print("rect_sp:", sign_tag(r), sign_tag(phi), "->", type(e).__name__, str(e))

# 4. Finite values & mathematical identities for all elementary functions
test_pts = [0.5 + 0.25j, -0.5 + 0.25j, 0.5 - 0.25j, -0.5 - 0.25j, 2 + 3j, -2 + 3j, 2 - 3j, -2 - 3j, 0.0, 2, -4]
for z in test_pts:
    z = complex(z)
    # sqrt(z)^2 == z
    s = cmath.sqrt(z)
    print("sqrt_id:", cmath.isclose(s * s, z, rel_tol=1e-12, abs_tol=1e-12))
    # exp(log(z)) == z (for z != 0)
    if z != 0:
        print("exp_log:", cmath.isclose(cmath.exp(cmath.log(z)), z, rel_tol=1e-12, abs_tol=1e-12))
        print("log10_id:", cmath.isclose(cmath.log10(z), cmath.log(z, 10), rel_tol=1e-12, abs_tol=1e-12))
        print("log_base:", cmath.isclose(cmath.log(z, 2 + 1j), cmath.log(z) / cmath.log(2 + 1j), rel_tol=1e-12, abs_tol=1e-12))
    # sin^2 + cos^2 == 1, tan == sin/cos
    si, co, ta = cmath.sin(z), cmath.cos(z), cmath.tan(z)
    print("trig_id:", cmath.isclose(si * si + co * co, 1, rel_tol=1e-12, abs_tol=1e-12), cmath.isclose(ta, si / co, rel_tol=1e-12, abs_tol=1e-12))
    # cosh^2 - sinh^2 == 1, tanh == sinh/cosh
    sh, ch, th = cmath.sinh(z), cmath.cosh(z), cmath.tanh(z)
    print("hyp_id:", cmath.isclose(ch * ch - sh * sh, 1, rel_tol=1e-12, abs_tol=1e-12), cmath.isclose(th, sh / ch, rel_tol=1e-12, abs_tol=1e-12))
    # inverse round-trips
    print(
        "inv_id:",
        cmath.isclose(cmath.cos(cmath.acos(z)), z, rel_tol=1e-11, abs_tol=1e-11),
        cmath.isclose(cmath.sin(cmath.asin(z)), z, rel_tol=1e-11, abs_tol=1e-11),
        cmath.isclose(cmath.tan(cmath.atan(z)), z, rel_tol=1e-11, abs_tol=1e-11),
        cmath.isclose(cmath.cosh(cmath.acosh(z)), z, rel_tol=1e-11, abs_tol=1e-11),
        cmath.isclose(cmath.sinh(cmath.asinh(z)), z, rel_tol=1e-11, abs_tol=1e-11),
        cmath.isclose(cmath.tanh(cmath.atanh(z)), z, rel_tol=1e-11, abs_tol=1e-11),
    )

# 5. Branch cuts and signed-zero parity on C99 Annex G special grid
grid = [float("-inf"), -1.0, -0.0, 0.0, 1.0, float("inf"), float("nan")]
for fn_name in [
    "sqrt", "exp", "log", "log10",
    "acos", "asin", "atan",
    "cos", "sin", "tan",
    "acosh", "asinh", "atanh",
    "cosh", "sinh", "tanh",
]:
    fn = getattr(cmath, fn_name)
    for a in grid:
        for b in grid:
            if a in (-1.0, 1.0) and b in (-1.0, 1.0):
                continue
            try:
                res = fn(complex(a, b))
                print("sp:", fn_name, sign_tag(a), sign_tag(b), "->", fmt_special(res))
            except Exception as e:
                print("sp:", fn_name, sign_tag(a), sign_tag(b), "->", type(e).__name__, str(e))

# 6. Branch cuts off the axes (e.g., ±2 ± 0j)
for fn_name in ["sqrt", "log", "acos", "asin", "acosh", "atanh"]:
    fn = getattr(cmath, fn_name)
    for z in [complex(-2.0, 0.0), complex(-2.0, -0.0), complex(2.0, 0.0), complex(2.0, -0.0)]:
        print("cut:", fn_name, fmt_c(z), "->", fmt_c(fn(z)))

# 7. Overflow & extreme magnitudes
for label, fn in [
    ("exp_ovf", lambda: cmath.exp(1000 + 0j)),
    ("cosh_ovf", lambda: cmath.cosh(1000 + 0j)),
    ("sinh_ovf", lambda: cmath.sinh(1000 + 0j)),
    ("polar_ovf", lambda: cmath.polar(complex(1.5e308, 1.5e308))),
    ("log_zero", lambda: cmath.log(0)),
    ("log_base1", lambda: cmath.log(2, 1)),
    ("log_base0", lambda: cmath.log(2, 0)),
]:
    try:
        print(label, fn())
    except Exception as e:
        print(label, type(e).__name__, str(e))

print("polar_large:", cmath.isclose(cmath.polar(complex(1e308, 1e308))[0], math.sqrt(2) * 1e308, rel_tol=1e-12))
print("log_large:", cmath.isclose(cmath.log(complex(1e308, 1e308)).real, math.log(1e308) + 0.5 * math.log(2), rel_tol=1e-12))
print("sqrt_subnormal:", cmath.isclose(cmath.sqrt(complex(1e-310, 0.0)), complex(1e-155, 0.0), rel_tol=1e-12))

# 8. isclose
print(
    "isclose:",
    cmath.isclose(1 + 2j, 1 + 2j),
    cmath.isclose(1 + 2j, 1 + 2.0000000001j),
    cmath.isclose(1 + 2j, 1 + 2.1j, rel_tol=0.1),
    cmath.isclose(0, 1e-10, abs_tol=1e-9),
    cmath.isclose(0, 1e-8, abs_tol=1e-9),
    cmath.isclose(cmath.inf, cmath.inf),
    cmath.isclose(cmath.inf, -cmath.inf),
    cmath.isclose(cmath.infj, cmath.infj),
    cmath.isclose(cmath.nan, cmath.nan),
    cmath.isclose(cmath.nanj, cmath.nanj),
)
for label, fn in [
    ("isclose_neg_rel", lambda: cmath.isclose(1, 2, rel_tol=-1e-9)),
    ("isclose_neg_abs", lambda: cmath.isclose(1, 2, abs_tol=-1e-9)),
]:
    try:
        fn()
    except Exception as e:
        print(label, type(e).__name__, str(e))

# 9. Custom numeric coercion (__complex__, __float__, __index__)
class MyComplex:
    def __complex__(self):
        return 3 + 4j

class MyFloat:
    def __float__(self):
        return 4.0

class MyIndex:
    def __index__(self):
        return 9

print("coerce:", cmath.polar(MyComplex()), cmath.sqrt(MyFloat()), cmath.sqrt(MyIndex()))
for bad in ["1", None, [1]]:
    try:
        cmath.sqrt(bad)
    except Exception as e:
        print("bad_type:", type(e).__name__, str(e))
