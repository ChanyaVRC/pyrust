//! The real-valued and binomial distributions of CPython 3.12.3's
//! `Lib/random.py`, generic over [`Arith`] and in the source's evaluation
//! order (so the same draws are consumed and the same errors raised).

use std::f64::consts::{E, PI, TAU};

use crate::error::Result;

use super::arith::Arith;
use super::arith::Cmp::{Eq, Ge, Gt, Le, Lt};
use super::arith::MathFn::{Acos, Cos, Exp, Fabs, Lgamma, Log, Log2, Sin, Sqrt};
use super::arith::Op::{Add, Div, Mod, Mul, Pow, Sub};

/// `NV_MAGICCONST`, `LOG4` and `SG_MAGICCONST`, computed at run time with the
/// platform `exp` / `log` exactly as the module body computes them.
pub(super) struct Consts {
    pub(super) nv_magicconst: f64,
    pub(super) log4: f64,
    pub(super) sg_magicconst: f64,
}

pub(super) fn consts() -> Consts {
    let half = std::hint::black_box(-0.5_f64);
    let two = std::hint::black_box(2.0_f64);
    let four = std::hint::black_box(4.0_f64);
    let four_and_half = std::hint::black_box(4.5_f64);
    Consts {
        nv_magicconst: 4.0 * half.exp() / two.sqrt(),
        log4: four.ln(),
        sg_magicconst: 1.0 + four_and_half.ln(),
    }
}

/// `a + (b - a) * self.random()`.
pub(super) fn uniform<A: Arith>(a: &mut A, lo: &A::N, hi: &A::N) -> Result<A::N> {
    let width = a.bin(hi, Sub, lo)?;
    let u = a.random()?;
    let scaled = a.bin(&width, Mul, &u)?;
    a.bin(lo, Add, &scaled)
}

/// The result of `triangular`: `return low` returns the argument object.
pub(super) enum Tri<N> {
    Value(N),
    Low,
}

pub(super) fn triangular<A: Arith>(
    a: &mut A,
    low: &A::N,
    high: &A::N,
    mode: Option<&A::N>,
) -> Result<Tri<A::N>> {
    let mut u = a.random()?;
    let mut c = match mode {
        None => a.lit(0.5),
        Some(mode) => {
            let c = a.catch_zero_division(|a| {
                let num = a.bin(mode, Sub, low)?;
                let den = a.bin(high, Sub, low)?;
                a.bin(&num, Div, &den)
            })?;
            match c {
                Some(c) => c,
                None => return Ok(Tri::Low),
            }
        }
    };
    let (mut low, mut high) = (low.clone(), high.clone());
    if a.cmp(&u, Gt, &c)? {
        let one = a.lit(1.0);
        u = a.bin(&one, Sub, &u)?;
        c = a.bin(&one, Sub, &c)?;
        std::mem::swap(&mut low, &mut high);
    }
    let width = a.bin(&high, Sub, &low)?;
    let uc = a.bin(&u, Mul, &c)?;
    let root = a.math(Sqrt, &uc)?;
    let scaled = a.bin(&width, Mul, &root)?;
    Ok(Tri::Value(a.bin(&low, Add, &scaled)?))
}

/// `mu + z * sigma`.
pub(super) fn affine<A: Arith>(a: &mut A, mu: &A::N, z: &A::N, sigma: &A::N) -> Result<A::N> {
    let scaled = a.bin(z, Mul, sigma)?;
    a.bin(mu, Add, &scaled)
}

/// Kinderman and Monahan's ratio-of-uniforms method.
pub(super) fn normalvariate<A: Arith>(a: &mut A, mu: &A::N, sigma: &A::N) -> Result<A::N> {
    let nv = a.lit(consts().nv_magicconst);
    let half = a.lit(0.5);
    let one = a.lit(1.0);
    let four = a.lit(4.0);
    let z = loop {
        let u1 = a.random()?;
        let r = a.random()?;
        let u2 = a.bin(&one, Sub, &r)?;
        let centred = a.bin(&u1, Sub, &half)?;
        let scaled = a.bin(&nv, Mul, &centred)?;
        let z = a.bin(&scaled, Div, &u2)?;
        let zz = a.bin(&z, Mul, &z)?;
        let zz = a.bin(&zz, Div, &four)?;
        let log = a.math(Log, &u2)?;
        let bound = a.neg(&log)?;
        if a.cmp(&zz, Le, &bound)? {
            break z;
        }
    };
    affine(a, mu, &z, sigma)
}

/// The Box-Muller pair `gauss()` computes: `(z, gauss_next)`.
pub(super) fn gauss_pair<A: Arith>(a: &mut A) -> Result<(A::N, A::N)> {
    let twopi = a.lit(TAU);
    let one = a.lit(1.0);
    let minus_two = a.lit(-2.0);
    let r = a.random()?;
    let x2pi = a.bin(&r, Mul, &twopi)?;
    let r = a.random()?;
    let tail = a.bin(&one, Sub, &r)?;
    let log = a.math(Log, &tail)?;
    let radicand = a.bin(&minus_two, Mul, &log)?;
    let g2rad = a.math(Sqrt, &radicand)?;
    let cos = a.math(Cos, &x2pi)?;
    let z = a.bin(&cos, Mul, &g2rad)?;
    let sin = a.math(Sin, &x2pi)?;
    let next = a.bin(&sin, Mul, &g2rad)?;
    Ok((z, next))
}

/// `-_log(1.0 - self.random()) / lambd`.
pub(super) fn expovariate<A: Arith>(a: &mut A, lambd: &A::N) -> Result<A::N> {
    let one = a.lit(1.0);
    let r = a.random()?;
    let tail = a.bin(&one, Sub, &r)?;
    let log = a.math(Log, &tail)?;
    let neg = a.neg(&log)?;
    a.bin(&neg, Div, lambd)
}

/// Fisher's algorithm for the von Mises distribution.
pub(super) fn vonmisesvariate<A: Arith>(a: &mut A, mu: &A::N, kappa: &A::N) -> Result<A::N> {
    let twopi = a.lit(TAU);
    let one = a.lit(1.0);
    let tiny = a.lit(1e-6);
    if a.cmp(kappa, Le, &tiny)? {
        let r = a.random()?;
        return a.bin(&twopi, Mul, &r);
    }
    let half = a.lit(0.5);
    let s = a.bin(&half, Div, kappa)?;
    let ss = a.bin(&s, Mul, &s)?;
    let radicand = a.bin(&one, Add, &ss)?;
    let root = a.math(Sqrt, &radicand)?;
    let r = a.bin(&s, Add, &root)?;
    let pi = a.lit(PI);
    let z = loop {
        let u1 = a.random()?;
        let angle = a.bin(&pi, Mul, &u1)?;
        let z = a.math(Cos, &angle)?;
        let rz = a.bin(&r, Add, &z)?;
        let d = a.bin(&z, Div, &rz)?;
        let u2 = a.random()?;
        let dd = a.bin(&d, Mul, &d)?;
        let first = a.bin(&one, Sub, &dd)?;
        if a.cmp(&u2, Lt, &first)? {
            break z;
        }
        let one_minus_d = a.bin(&one, Sub, &d)?;
        let exp_d = a.math(Exp, &d)?;
        let second = a.bin(&one_minus_d, Mul, &exp_d)?;
        if a.cmp(&u2, Le, &second)? {
            break z;
        }
    };
    let q = a.bin(&one, Div, &r)?;
    let num = a.bin(&q, Add, &z)?;
    let qz = a.bin(&q, Mul, &z)?;
    let den = a.bin(&one, Add, &qz)?;
    let f = a.bin(&num, Div, &den)?;
    let u3 = a.random()?;
    let up = a.cmp(&u3, Gt, &half)?;
    let acos = a.math(Acos, &f)?;
    let angle = a.bin(mu, if up { Add } else { Sub }, &acos)?;
    a.bin(&angle, Mod, &twopi)
}

/// Cheng's (alpha > 1), exponential (alpha == 1) and Kennedy & Gentle's GS
/// (alpha < 1) gamma generators; the caller has checked `alpha, beta > 0`.
pub(super) fn gammavariate<A: Arith>(a: &mut A, alpha: &A::N, beta: &A::N) -> Result<A::N> {
    let one = a.lit(1.0);
    if a.cmp(alpha, Gt, &one)? {
        let k = consts();
        let two = a.lit(2.0);
        let twice = a.bin(&two, Mul, alpha)?;
        let radicand = a.bin(&twice, Sub, &one)?;
        let ainv = a.math(Sqrt, &radicand)?;
        let log4 = a.lit(k.log4);
        let bbb = a.bin(alpha, Sub, &log4)?;
        let ccc = a.bin(alpha, Add, &ainv)?;
        let lower = a.lit(1e-7);
        let upper = a.lit(0.9999999);
        let sg = a.lit(k.sg_magicconst);
        let four_and_half = a.lit(4.5);
        let zero = a.lit(0.0);
        loop {
            let u1 = a.random()?;
            if !(a.cmp(&lower, Lt, &u1)? && a.cmp(&u1, Lt, &upper)?) {
                continue;
            }
            let r = a.random()?;
            let u2 = a.bin(&one, Sub, &r)?;
            let complement = a.bin(&one, Sub, &u1)?;
            let ratio = a.bin(&u1, Div, &complement)?;
            let log = a.math(Log, &ratio)?;
            let v = a.bin(&log, Div, &ainv)?;
            let exp_v = a.math(Exp, &v)?;
            let x = a.bin(alpha, Mul, &exp_v)?;
            let uu = a.bin(&u1, Mul, &u1)?;
            let z = a.bin(&uu, Mul, &u2)?;
            let cv = a.bin(&ccc, Mul, &v)?;
            let sum = a.bin(&bbb, Add, &cv)?;
            let r = a.bin(&sum, Sub, &x)?;
            let shifted = a.bin(&r, Add, &sg)?;
            let z45 = a.bin(&four_and_half, Mul, &z)?;
            let lhs = a.bin(&shifted, Sub, &z45)?;
            if a.cmp(&lhs, Ge, &zero)? {
                return a.bin(&x, Mul, beta);
            }
            let log_z = a.math(Log, &z)?;
            if a.cmp(&r, Ge, &log_z)? {
                return a.bin(&x, Mul, beta);
            }
        }
    }
    if a.cmp(alpha, Eq, &one)? {
        let r = a.random()?;
        let tail = a.bin(&one, Sub, &r)?;
        let log = a.math(Log, &tail)?;
        let neg = a.neg(&log)?;
        return a.bin(&neg, Mul, beta);
    }
    let e = a.lit(E);
    let x = loop {
        let u = a.random()?;
        let shifted = a.bin(&e, Add, alpha)?;
        let b = a.bin(&shifted, Div, &e)?;
        let p = a.bin(&b, Mul, &u)?;
        let x = if a.cmp(&p, Le, &one)? {
            let exponent = a.bin(&one, Div, alpha)?;
            a.bin(&p, Pow, &exponent)?
        } else {
            let rest = a.bin(&b, Sub, &p)?;
            let ratio = a.bin(&rest, Div, alpha)?;
            let log = a.math(Log, &ratio)?;
            a.neg(&log)?
        };
        let u1 = a.random()?;
        if a.cmp(&p, Gt, &one)? {
            let exponent = a.bin(alpha, Sub, &one)?;
            let bound = a.bin(&x, Pow, &exponent)?;
            if a.cmp(&u1, Le, &bound)? {
                break x;
            }
        } else {
            let neg = a.neg(&x)?;
            let bound = a.math(Exp, &neg)?;
            if a.cmp(&u1, Le, &bound)? {
                break x;
            }
        }
    };
    a.bin(&x, Mul, beta)
}

/// `(1.0 - self.random()) ** (-1.0 / alpha)`.
pub(super) fn paretovariate<A: Arith>(a: &mut A, alpha: &A::N) -> Result<A::N> {
    let one = a.lit(1.0);
    let r = a.random()?;
    let u = a.bin(&one, Sub, &r)?;
    let minus_one = a.lit(-1.0);
    let exponent = a.bin(&minus_one, Div, alpha)?;
    a.bin(&u, Pow, &exponent)
}

/// `alpha * (-_log(1.0 - self.random())) ** (1.0 / beta)`.
pub(super) fn weibullvariate<A: Arith>(a: &mut A, alpha: &A::N, beta: &A::N) -> Result<A::N> {
    let one = a.lit(1.0);
    let r = a.random()?;
    let u = a.bin(&one, Sub, &r)?;
    let log = a.math(Log, &u)?;
    let base = a.neg(&log)?;
    let exponent = a.bin(&one, Div, beta)?;
    let power = a.bin(&base, Pow, &exponent)?;
    a.bin(alpha, Mul, &power)
}

/// The BTRS quantities CPython computes once, on the first acceptance test
/// (`setup_complete`).
#[derive(Clone)]
struct BtrsSetup<N> {
    alpha: N,
    lpq: N,
    m: N,
    h: N,
}

/// The two `binomialvariate` generators once `0 < p <= 0.5` and `n != 1`:
/// Devroye's geometric method (BG) for `n * p < 10` and Hörmann's BTRS
/// otherwise.  The result is an `int`; `None` is the source's failing
/// `assert n*p >= 10.0 and p <= 0.5` (a NaN `p`).
pub(super) fn binomial<A: Arith>(a: &mut A, n: &A::N, p: &A::N) -> Result<Option<A::N>> {
    let ten = a.lit(10.0);
    let one = a.lit(1.0);
    let int_one = a.int(1);
    let np = a.bin(n, Mul, p)?;
    if a.cmp(&np, Lt, &ten)? {
        let mut x = a.int(0);
        let mut y = a.int(0);
        let q = a.bin(&one, Sub, p)?;
        let c = a.math(Log2, &q)?;
        if !a.truthy(&c)? {
            return Ok(Some(x));
        }
        loop {
            let r = a.random()?;
            let log = a.math(Log2, &r)?;
            let ratio = a.bin(&log, Div, &c)?;
            let floor = a.floor(&ratio)?;
            let step = a.bin(&floor, Add, &int_one)?;
            y = a.bin(&y, Add, &step)?;
            if a.cmp(&y, Gt, n)? {
                return Ok(Some(x));
            }
            x = a.bin(&x, Add, &int_one)?;
        }
    }
    let np = a.bin(n, Mul, p)?;
    let half = a.lit(0.5);
    if !(a.cmp(&np, Ge, &ten)? && a.cmp(p, Le, &half)?) {
        return Ok(None);
    }
    let q = a.bin(&one, Sub, p)?;
    let npq = a.bin(&np, Mul, &q)?;
    let spq = a.math(Sqrt, &npq)?;
    let (c115, c253) = (a.lit(1.15), a.lit(2.53));
    let t = a.bin(&c253, Mul, &spq)?;
    let b = a.bin(&c115, Add, &t)?;
    let (c0873, c0248, c001) = (a.lit(-0.0873), a.lit(0.0248), a.lit(0.01));
    let t = a.bin(&c0248, Mul, &b)?;
    let t = a.bin(&c0873, Add, &t)?;
    let t2 = a.bin(&c001, Mul, p)?;
    let alpha_a = a.bin(&t, Add, &t2)?;
    let np = a.bin(n, Mul, p)?;
    let c = a.bin(&np, Add, &half)?;
    let (c092, c42) = (a.lit(0.92), a.lit(4.2));
    let t = a.bin(&c42, Div, &b)?;
    let vr = a.bin(&c092, Sub, &t)?;
    let zero = a.int(0);
    let two = a.lit(2.0);
    let c007 = a.lit(0.07);
    let mut setup: Option<BtrsSetup<A::N>> = None;
    loop {
        let u = a.random()?;
        let u = a.bin(&u, Sub, &half)?;
        let abs_u = a.math(Fabs, &u)?;
        let us = a.bin(&half, Sub, &abs_u)?;
        let t = a.bin(&two, Mul, &alpha_a)?;
        let t = a.bin(&t, Div, &us)?;
        let t = a.bin(&t, Add, &b)?;
        let t = a.bin(&t, Mul, &u)?;
        let t = a.bin(&t, Add, &c)?;
        let k = a.floor(&t)?;
        if a.cmp(&k, Lt, &zero)? || a.cmp(&k, Gt, n)? {
            continue;
        }
        let v = a.random()?;
        if a.cmp(&us, Ge, &c007)? && a.cmp(&v, Le, &vr)? {
            return Ok(Some(k));
        }
        if setup.is_none() {
            let (c283, c51) = (a.lit(2.83), a.lit(5.1));
            let t = a.bin(&c51, Div, &b)?;
            let t = a.bin(&c283, Add, &t)?;
            let alpha = a.bin(&t, Mul, &spq)?;
            let q = a.bin(&one, Sub, p)?;
            let ratio = a.bin(p, Div, &q)?;
            let lpq = a.math(Log, &ratio)?;
            let n1 = a.bin(n, Add, &int_one)?;
            let t = a.bin(&n1, Mul, p)?;
            let m = a.floor(&t)?;
            let m1 = a.bin(&m, Add, &int_one)?;
            let g1 = a.math(Lgamma, &m1)?;
            let nm = a.bin(n, Sub, &m)?;
            let nm1 = a.bin(&nm, Add, &int_one)?;
            let g2 = a.math(Lgamma, &nm1)?;
            let h = a.bin(&g1, Add, &g2)?;
            setup = Some(BtrsSetup { alpha, lpq, m, h });
        }
        let BtrsSetup { alpha, lpq, m, h } = setup.clone().expect("set up above");
        let us2 = a.bin(&us, Mul, &us)?;
        let t = a.bin(&alpha_a, Div, &us2)?;
        let t = a.bin(&t, Add, &b)?;
        let t = a.bin(&alpha, Div, &t)?;
        let v = a.bin(&v, Mul, &t)?;
        let lhs = a.math(Log, &v)?;
        let k1 = a.bin(&k, Add, &int_one)?;
        let g1 = a.math(Lgamma, &k1)?;
        let rhs = a.bin(&h, Sub, &g1)?;
        let nk = a.bin(n, Sub, &k)?;
        let nk1 = a.bin(&nk, Add, &int_one)?;
        let g2 = a.math(Lgamma, &nk1)?;
        let rhs = a.bin(&rhs, Sub, &g2)?;
        let km = a.bin(&k, Sub, &m)?;
        let t = a.bin(&km, Mul, &lpq)?;
        let rhs = a.bin(&rhs, Add, &t)?;
        if a.cmp(&lhs, Le, &rhs)? {
            return Ok(Some(k));
        }
    }
}
