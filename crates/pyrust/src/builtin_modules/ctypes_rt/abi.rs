//! Pure-Rust cross-platform C ABI call dispatcher and static `CFUNCTYPE`
//! callback trampoline pool (ZERO `libffi`, ZERO JIT/W^X, works across
//! Linux x86_64/aarch64, macOS x86_64/aarch64, and Windows x64).

use std::cell::{Cell, RefCell};

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

use super::dynlib;
use super::types::{CTypeInfo, PrimKind};

// ── Active Interpreter TLS Guard for C -> Python Callbacks ───────────────────

thread_local! {
    static ACTIVE_INTERP: Cell<*mut Interpreter> = const { Cell::new(std::ptr::null_mut()) };
}

struct ActiveInterpGuard {
    prev: *mut Interpreter,
}

impl ActiveInterpGuard {
    fn push(interp: &mut Interpreter) -> Self {
        let ptr: *mut Interpreter = interp;
        let prev = ACTIVE_INTERP.with(|c| c.replace(ptr));
        Self { prev }
    }
}

impl Drop for ActiveInterpGuard {
    fn drop(&mut self) {
        ACTIVE_INTERP.with(|c| c.set(self.prev));
    }
}

// ── ABI Value Classification ─────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AbiKind {
    Int,
    F32,
    F64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AbiVal {
    Int(u64),
    F32(f32),
    F64(f64),
}

impl AbiVal {
    #[inline]
    pub(crate) fn as_int_bits(self) -> u64 {
        match self {
            Self::Int(v) => v,
            Self::F32(v) => v.to_bits() as u64,
            Self::F64(v) => v.to_bits(),
        }
    }

    #[inline]
    pub(crate) fn as_f32(self) -> f32 {
        match self {
            Self::F32(v) => v,
            Self::F64(v) => v as f32,
            Self::Int(v) => f32::from_bits(v as u32),
        }
    }

    #[inline]
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            Self::F64(v) => v,
            Self::F32(v) => f64::from_bits(v.to_bits() as u64),
            Self::Int(v) => f64::from_bits(v),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AbiRetKind {
    Void,
    Int,
    F32,
    F64,
    StructBytes(usize),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AbiRet {
    Void,
    Int(u64),
    F32(f32),
    F64(f64),
    StructBytes([u8; 16], usize),
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct PairU64(u64, u64);

// ── Exact Mixed-Type Dispatch Macros (0..=4 args) ────────────────────────────

macro_rules! call_fn_1 {
    ($addr:expr, $a0:expr, $ret_ty:ty) => {{
        match $a0 {
            AbiVal::Int(v0) => {
                let f: extern "C" fn(u64) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f(v0)
            }
            AbiVal::F32(v0) => {
                let f: extern "C" fn(f32) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f(v0)
            }
            AbiVal::F64(v0) => {
                let f: extern "C" fn(f64) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f(v0)
            }
        }
    }};
}

macro_rules! call_fn_2_inner {
    ($addr:expr, $t0:ty, $v0:expr, $a1:expr, $ret_ty:ty) => {{
        match $a1 {
            AbiVal::Int(v1) => {
                let f: extern "C" fn($t0, u64) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f($v0, v1)
            }
            AbiVal::F32(v1) => {
                let f: extern "C" fn($t0, f32) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f($v0, v1)
            }
            AbiVal::F64(v1) => {
                let f: extern "C" fn($t0, f64) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f($v0, v1)
            }
        }
    }};
}

macro_rules! call_fn_2 {
    ($addr:expr, $a0:expr, $a1:expr, $ret_ty:ty) => {{
        match $a0 {
            AbiVal::Int(v0) => call_fn_2_inner!($addr, u64, v0, $a1, $ret_ty),
            AbiVal::F32(v0) => call_fn_2_inner!($addr, f32, v0, $a1, $ret_ty),
            AbiVal::F64(v0) => call_fn_2_inner!($addr, f64, v0, $a1, $ret_ty),
        }
    }};
}

macro_rules! call_fn_3_b {
    ($addr:expr, $t0:ty, $v0:expr, $t1:ty, $v1:expr, $a2:expr, $ret_ty:ty) => {{
        match $a2 {
            AbiVal::Int(v2) => {
                let f: extern "C" fn($t0, $t1, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                f($v0, $v1, v2)
            }
            AbiVal::F32(v2) => {
                let f: extern "C" fn($t0, $t1, f32) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                f($v0, $v1, v2)
            }
            AbiVal::F64(v2) => {
                let f: extern "C" fn($t0, $t1, f64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                f($v0, $v1, v2)
            }
        }
    }};
}

macro_rules! call_fn_3_a {
    ($addr:expr, $t0:ty, $v0:expr, $a1:expr, $a2:expr, $ret_ty:ty) => {{
        match $a1 {
            AbiVal::Int(v1) => call_fn_3_b!($addr, $t0, $v0, u64, v1, $a2, $ret_ty),
            AbiVal::F32(v1) => call_fn_3_b!($addr, $t0, $v0, f32, v1, $a2, $ret_ty),
            AbiVal::F64(v1) => call_fn_3_b!($addr, $t0, $v0, f64, v1, $a2, $ret_ty),
        }
    }};
}

macro_rules! call_fn_3 {
    ($addr:expr, $a0:expr, $a1:expr, $a2:expr, $ret_ty:ty) => {{
        match $a0 {
            AbiVal::Int(v0) => call_fn_3_a!($addr, u64, v0, $a1, $a2, $ret_ty),
            AbiVal::F32(v0) => call_fn_3_a!($addr, f32, v0, $a1, $a2, $ret_ty),
            AbiVal::F64(v0) => call_fn_3_a!($addr, f64, v0, $a1, $a2, $ret_ty),
        }
    }};
}

macro_rules! call_fn_4_c {
    ($addr:expr, $t0:ty, $v0:expr, $t1:ty, $v1:expr, $t2:ty, $v2:expr, $a3:expr, $ret_ty:ty) => {{
        match $a3 {
            AbiVal::Int(v3) => {
                let f: extern "C" fn($t0, $t1, $t2, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                f($v0, $v1, $v2, v3)
            }
            AbiVal::F32(v3) => {
                let f: extern "C" fn($t0, $t1, $t2, f32) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                f($v0, $v1, $v2, v3)
            }
            AbiVal::F64(v3) => {
                let f: extern "C" fn($t0, $t1, $t2, f64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                f($v0, $v1, $v2, v3)
            }
        }
    }};
}

macro_rules! call_fn_4_b {
    ($addr:expr, $t0:ty, $v0:expr, $t1:ty, $v1:expr, $a2:expr, $a3:expr, $ret_ty:ty) => {{
        match $a2 {
            AbiVal::Int(v2) => call_fn_4_c!($addr, $t0, $v0, $t1, $v1, u64, v2, $a3, $ret_ty),
            AbiVal::F32(v2) => call_fn_4_c!($addr, $t0, $v0, $t1, $v1, f32, v2, $a3, $ret_ty),
            AbiVal::F64(v2) => call_fn_4_c!($addr, $t0, $v0, $t1, $v1, f64, v2, $a3, $ret_ty),
        }
    }};
}

macro_rules! call_fn_4_a {
    ($addr:expr, $t0:ty, $v0:expr, $a1:expr, $a2:expr, $a3:expr, $ret_ty:ty) => {{
        match $a1 {
            AbiVal::Int(v1) => call_fn_4_b!($addr, $t0, $v0, u64, v1, $a2, $a3, $ret_ty),
            AbiVal::F32(v1) => call_fn_4_b!($addr, $t0, $v0, f32, v1, $a2, $a3, $ret_ty),
            AbiVal::F64(v1) => call_fn_4_b!($addr, $t0, $v0, f64, v1, $a2, $a3, $ret_ty),
        }
    }};
}

macro_rules! call_fn_4 {
    ($addr:expr, $a0:expr, $a1:expr, $a2:expr, $a3:expr, $ret_ty:ty) => {{
        match $a0 {
            AbiVal::Int(v0) => call_fn_4_a!($addr, u64, v0, $a1, $a2, $a3, $ret_ty),
            AbiVal::F32(v0) => call_fn_4_a!($addr, f32, v0, $a1, $a2, $a3, $ret_ty),
            AbiVal::F64(v0) => call_fn_4_a!($addr, f64, v0, $a1, $a2, $a3, $ret_ty),
        }
    }};
}

// On Windows x64, args 0..4 go into rcx/rdx/r8/r9 or xmm0..xmm3 and args 4..12
// ALWAYS go into 8-byte stack slots as raw 64-bit values regardless of type.
#[cfg(windows)]
macro_rules! call_win64_many_c {
    ($addr:expr, $t0:ty, $v0:expr, $t1:ty, $v1:expr, $t2:ty, $v2:expr, $a3:expr, $s:expr, $ret_ty:ty) => {{
        match $a3 {
            AbiVal::Int(v3) => {
                let f: extern "C" fn(
                    $t0,
                    $t1,
                    $t2,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f(
                    $v0, $v1, $v2, v3, $s[0], $s[1], $s[2], $s[3], $s[4], $s[5], $s[6], $s[7],
                )
            }
            AbiVal::F32(v3) => {
                let f: extern "C" fn(
                    $t0,
                    $t1,
                    $t2,
                    f32,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f(
                    $v0, $v1, $v2, v3, $s[0], $s[1], $s[2], $s[3], $s[4], $s[5], $s[6], $s[7],
                )
            }
            AbiVal::F64(v3) => {
                let f: extern "C" fn(
                    $t0,
                    $t1,
                    $t2,
                    f64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                f(
                    $v0, $v1, $v2, v3, $s[0], $s[1], $s[2], $s[3], $s[4], $s[5], $s[6], $s[7],
                )
            }
        }
    }};
}

#[cfg(windows)]
macro_rules! call_win64_many_b {
    ($addr:expr, $t0:ty, $v0:expr, $t1:ty, $v1:expr, $a2:expr, $a3:expr, $s:expr, $ret_ty:ty) => {{
        match $a2 {
            AbiVal::Int(v2) => {
                call_win64_many_c!($addr, $t0, $v0, $t1, $v1, u64, v2, $a3, $s, $ret_ty)
            }
            AbiVal::F32(v2) => {
                call_win64_many_c!($addr, $t0, $v0, $t1, $v1, f32, v2, $a3, $s, $ret_ty)
            }
            AbiVal::F64(v2) => {
                call_win64_many_c!($addr, $t0, $v0, $t1, $v1, f64, v2, $a3, $s, $ret_ty)
            }
        }
    }};
}

#[cfg(windows)]
macro_rules! call_win64_many_a {
    ($addr:expr, $t0:ty, $v0:expr, $a1:expr, $a2:expr, $a3:expr, $s:expr, $ret_ty:ty) => {{
        match $a1 {
            AbiVal::Int(v1) => call_win64_many_b!($addr, $t0, $v0, u64, v1, $a2, $a3, $s, $ret_ty),
            AbiVal::F32(v1) => call_win64_many_b!($addr, $t0, $v0, f32, v1, $a2, $a3, $s, $ret_ty),
            AbiVal::F64(v1) => call_win64_many_b!($addr, $t0, $v0, f64, v1, $a2, $a3, $s, $ret_ty),
        }
    }};
}

#[cfg(windows)]
macro_rules! call_win64_many {
    ($addr:expr, $args:expr, $ret_ty:ty) => {{
        let mut s = [0u64; 8];
        for (i, a) in $args.iter().skip(4).take(8).enumerate() {
            s[i] = a.as_int_bits();
        }
        match $args[0] {
            AbiVal::Int(v0) => {
                call_win64_many_a!($addr, u64, v0, $args[1], $args[2], $args[3], s, $ret_ty)
            }
            AbiVal::F32(v0) => {
                call_win64_many_a!($addr, f32, v0, $args[1], $args[2], $args[3], s, $ret_ty)
            }
            AbiVal::F64(v0) => {
                call_win64_many_a!($addr, f64, v0, $args[1], $args[2], $args[3], s, $ret_ty)
            }
        }
    }};
}

// Generic all-Int exact arity dispatcher (0..=16 args) for any return type `R`.
macro_rules! dispatch_all_int {
    ($addr:expr, $args:expr, $ret_ty:ty) => {{
        let g = |i: usize| $args[i].as_int_bits();
        match $args.len() {
            0 => {
                let f: extern "C" fn() -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f())
            }
            1 => {
                let f: extern "C" fn(u64) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(g(0)))
            }
            2 => {
                let f: extern "C" fn(u64, u64) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1)))
            }
            3 => {
                let f: extern "C" fn(u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1), g(2)))
            }
            4 => {
                let f: extern "C" fn(u64, u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1), g(2), g(3)))
            }
            5 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1), g(2), g(3), g(4)))
            }
            6 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1), g(2), g(3), g(4), g(5)))
            }
            7 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1), g(2), g(3), g(4), g(5), g(6)))
            }
            8 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1), g(2), g(3), g(4), g(5), g(6), g(7)))
            }
            9 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(g(0), g(1), g(2), g(3), g(4), g(5), g(6), g(7), g(8)))
            }
            10 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> $ret_ty =
                    unsafe { std::mem::transmute($addr) };
                Ok(f(
                    g(0),
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6),
                    g(7),
                    g(8),
                    g(9),
                ))
            }
            11 => {
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(
                    g(0),
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6),
                    g(7),
                    g(8),
                    g(9),
                    g(10),
                ))
            }
            12 => {
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(
                    g(0),
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6),
                    g(7),
                    g(8),
                    g(9),
                    g(10),
                    g(11),
                ))
            }
            13 => {
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(
                    g(0),
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6),
                    g(7),
                    g(8),
                    g(9),
                    g(10),
                    g(11),
                    g(12),
                ))
            }
            14 => {
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(
                    g(0),
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6),
                    g(7),
                    g(8),
                    g(9),
                    g(10),
                    g(11),
                    g(12),
                    g(13),
                ))
            }
            15 => {
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(
                    g(0),
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6),
                    g(7),
                    g(8),
                    g(9),
                    g(10),
                    g(11),
                    g(12),
                    g(13),
                    g(14),
                ))
            }
            16 => {
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                Ok(f(
                    g(0),
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6),
                    g(7),
                    g(8),
                    g(9),
                    g(10),
                    g(11),
                    g(12),
                    g(13),
                    g(14),
                    g(15),
                ))
            }
            n => Err(PyError::named(
                "TypeError",
                format!("ctypes native ABI supports at most 16 arguments, got {n}"),
            )),
        }
    }};
}

macro_rules! dispatch_typed {
    ($addr:expr, $args:expr, $ret_ty:ty) => {{
        let all_int = $args.iter().all(|a| matches!(a, AbiVal::Int(_)));
        if all_int {
            dispatch_all_int!($addr, $args, $ret_ty)
        } else {
            match $args.len() {
                0 => {
                    let f: extern "C" fn() -> $ret_ty = unsafe { std::mem::transmute($addr) };
                    Ok(f())
                }
                1 => Ok(call_fn_1!($addr, $args[0], $ret_ty)),
                2 => Ok(call_fn_2!($addr, $args[0], $args[1], $ret_ty)),
                3 => Ok(call_fn_3!($addr, $args[0], $args[1], $args[2], $ret_ty)),
                4 => Ok(call_fn_4!(
                    $addr, $args[0], $args[1], $args[2], $args[3], $ret_ty
                )),
                _ => {
                    #[cfg(windows)]
                    {
                        if $args.len() <= 12 {
                            Ok(call_win64_many!($addr, $args, $ret_ty))
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "ctypes native ABI supports at most 12 mixed float/int arguments, got {}",
                                    $args.len()
                                ),
                            ))
                        }
                    }
                    #[cfg(unix)]
                    {
                        // System V AMD64 (6 int regs + 8 xmm regs) and AAPCS64 (8 int regs + 8 v regs)
                        // allocate integer and floating-point registers independently.
                        let mut ints = [0u64; 6];
                        let mut floats = [0.0f64; 8];
                        let mut ni = 0usize;
                        let mut nf = 0usize;
                        let mut ok = true;
                        for a in $args {
                            match *a {
                                AbiVal::Int(v) => {
                                    if ni < 6 {
                                        ints[ni] = v;
                                        ni += 1;
                                    } else {
                                        ok = false;
                                        break;
                                    }
                                }
                                AbiVal::F64(v) => {
                                    if nf < 8 {
                                        floats[nf] = v;
                                        nf += 1;
                                    } else {
                                        ok = false;
                                        break;
                                    }
                                }
                                AbiVal::F32(v) => {
                                    if nf < 8 {
                                        floats[nf] = f64::from_bits(v.to_bits() as u64);
                                        nf += 1;
                                    } else {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                        }
                        if ok {
                            let f: extern "C" fn(
                                u64,
                                u64,
                                u64,
                                u64,
                                u64,
                                u64,
                                f64,
                                f64,
                                f64,
                                f64,
                                f64,
                                f64,
                                f64,
                                f64,
                            ) -> $ret_ty = unsafe { std::mem::transmute($addr) };
                            Ok(f(
                                ints[0], ints[1], ints[2], ints[3], ints[4], ints[5], floats[0],
                                floats[1], floats[2], floats[3], floats[4], floats[5], floats[6],
                                floats[7],
                            ))
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "ctypes native ABI exceeded mixed register limit ({ni} ints, {nf} floats)"
                                ),
                            ))
                        }
                    }
                }
            }
        }
    }};
}

/// Invoke a native C function at `addr` with prepared `args`, synchronizing
/// `errno` / `LastError` and exposing `interp` to any re-entrant `CFUNCTYPE`
/// callbacks triggered during the C call.
pub(crate) fn invoke_c_function(
    interp: &mut Interpreter,
    addr: usize,
    args: &[AbiVal],
    ret_kind: AbiRetKind,
    use_errno: bool,
    use_last_error: bool,
) -> Result<AbiRet> {
    if addr == 0 {
        return Err(PyError::named("ValueError", "NULL function pointer"));
    }

    let _guard = ActiveInterpGuard::push(interp);

    if use_errno {
        dynlib::sync_os_errno_before_call();
    }
    #[cfg(windows)]
    if use_last_error {
        dynlib::sync_os_last_error_before_call();
    }
    #[cfg(not(windows))]
    let _ = use_last_error;

    let result = match ret_kind {
        AbiRetKind::Void => {
            let _: u64 = dispatch_typed!(addr, args, u64)?;
            Ok(AbiRet::Void)
        }
        AbiRetKind::Int => {
            let r: u64 = dispatch_typed!(addr, args, u64)?;
            Ok(AbiRet::Int(r))
        }
        AbiRetKind::F32 => {
            let r: f32 = dispatch_typed!(addr, args, f32)?;
            Ok(AbiRet::F32(r))
        }
        AbiRetKind::F64 => {
            let r: f64 = dispatch_typed!(addr, args, f64)?;
            Ok(AbiRet::F64(r))
        }
        AbiRetKind::StructBytes(sz) if sz <= 8 => {
            let r: u64 = dispatch_typed!(addr, args, u64)?;
            let mut buf = [0u8; 16];
            buf[..8].copy_from_slice(&r.to_ne_bytes());
            Ok(AbiRet::StructBytes(buf, sz))
        }
        AbiRetKind::StructBytes(sz) if sz <= 16 => {
            #[cfg(unix)]
            {
                let pair: PairU64 = dispatch_typed!(addr, args, PairU64)?;
                let mut buf = [0u8; 16];
                buf[..8].copy_from_slice(&pair.0.to_ne_bytes());
                buf[8..16].copy_from_slice(&pair.1.to_ne_bytes());
                Ok(AbiRet::StructBytes(buf, sz))
            }
            #[cfg(windows)]
            {
                // On Win64, structs > 8 bytes are returned via hidden first pointer arg.
                let mut buf = [0u8; 16];
                let mut shifted = Vec::with_capacity(args.len() + 1);
                shifted.push(AbiVal::Int(buf.as_mut_ptr() as usize as u64));
                shifted.extend_from_slice(args);
                let _: u64 = dispatch_typed!(addr, &shifted, u64)?;
                Ok(AbiRet::StructBytes(buf, sz))
            }
        }
        AbiRetKind::StructBytes(sz) => Err(PyError::named(
            "TypeError",
            format!(
                "returning C structs larger than 16 bytes ({sz} bytes) by value is not supported"
            ),
        )),
    };

    if use_errno {
        dynlib::sync_os_errno_after_call();
    }
    #[cfg(windows)]
    if use_last_error {
        dynlib::sync_os_last_error_after_call();
    }

    result
}

// ── Static `CFUNCTYPE` Callback Trampoline Pool ──────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CallbackPoolKind {
    Int4ToInt,
    Int8ToInt,
    Int4ToF64,
    Int4ToF32,
    F64ToF64,
    F64ToInt,
    F32ToF32,
}

#[derive(Clone)]
struct CallbackSlot {
    callable: Value,
    restype: Option<Value>,
    argtypes: Vec<Value>,
}

const INT4_RET_INT_COUNT: usize = 32;
const INT8_RET_INT_COUNT: usize = 16;
const INT4_RET_F64_COUNT: usize = 8;
const INT4_RET_F32_COUNT: usize = 8;
const F64_RET_F64_COUNT: usize = 8;
const F64_RET_INT_COUNT: usize = 8;
const F32_RET_F32_COUNT: usize = 8;

struct CallbackRegistry {
    int4_ret_int: [Option<CallbackSlot>; INT4_RET_INT_COUNT],
    int8_ret_int: [Option<CallbackSlot>; INT8_RET_INT_COUNT],
    int4_ret_f64: [Option<CallbackSlot>; INT4_RET_F64_COUNT],
    int4_ret_f32: [Option<CallbackSlot>; INT4_RET_F32_COUNT],
    f64_ret_f64: [Option<CallbackSlot>; F64_RET_F64_COUNT],
    f64_ret_int: [Option<CallbackSlot>; F64_RET_INT_COUNT],
    f32_ret_f32: [Option<CallbackSlot>; F32_RET_F32_COUNT],
}

impl CallbackRegistry {
    const fn new() -> Self {
        Self {
            int4_ret_int: [const { None }; INT4_RET_INT_COUNT],
            int8_ret_int: [const { None }; INT8_RET_INT_COUNT],
            int4_ret_f64: [const { None }; INT4_RET_F64_COUNT],
            int4_ret_f32: [const { None }; INT4_RET_F32_COUNT],
            f64_ret_f64: [const { None }; F64_RET_F64_COUNT],
            f64_ret_int: [const { None }; F64_RET_INT_COUNT],
            f32_ret_f32: [const { None }; F32_RET_F32_COUNT],
        }
    }

    fn slice_mut(&mut self, pool: CallbackPoolKind) -> &mut [Option<CallbackSlot>] {
        match pool {
            CallbackPoolKind::Int4ToInt => &mut self.int4_ret_int,
            CallbackPoolKind::Int8ToInt => &mut self.int8_ret_int,
            CallbackPoolKind::Int4ToF64 => &mut self.int4_ret_f64,
            CallbackPoolKind::Int4ToF32 => &mut self.int4_ret_f32,
            CallbackPoolKind::F64ToF64 => &mut self.f64_ret_f64,
            CallbackPoolKind::F64ToInt => &mut self.f64_ret_int,
            CallbackPoolKind::F32ToF32 => &mut self.f32_ret_f32,
        }
    }

    fn get(&self, pool: CallbackPoolKind, idx: usize) -> Option<CallbackSlot> {
        let slice: &[Option<CallbackSlot>] = match pool {
            CallbackPoolKind::Int4ToInt => &self.int4_ret_int,
            CallbackPoolKind::Int8ToInt => &self.int8_ret_int,
            CallbackPoolKind::Int4ToF64 => &self.int4_ret_f64,
            CallbackPoolKind::Int4ToF32 => &self.int4_ret_f32,
            CallbackPoolKind::F64ToF64 => &self.f64_ret_f64,
            CallbackPoolKind::F64ToInt => &self.f64_ret_int,
            CallbackPoolKind::F32ToF32 => &self.f32_ret_f32,
        };
        slice.get(idx).and_then(Clone::clone)
    }
}

pyrust_core::gil_local! {
    static CALLBACK_REGISTRY: RefCell<CallbackRegistry> = RefCell::new(CallbackRegistry::new());
}

/// RAII handle owning a callback trampoline slot; frees the slot on drop.
pub(crate) struct CallbackSlotHandle {
    pool: CallbackPoolKind,
    index: usize,
}

impl Drop for CallbackSlotHandle {
    fn drop(&mut self) {
        let _ = CALLBACK_REGISTRY.try_with(|reg| {
            if let Ok(mut guard) = reg.try_borrow_mut() {
                let slice = guard.slice_mut(self.pool);
                if let Some(slot) = slice.get_mut(self.index) {
                    *slot = None;
                }
            }
        });
    }
}

fn run_callback(pool: CallbackPoolKind, idx: usize, raw_args: &[AbiVal]) -> AbiVal {
    let slot_opt = CALLBACK_REGISTRY.with(|reg| reg.borrow().get(pool, idx));
    let Some(slot) = slot_opt else {
        return AbiVal::Int(0);
    };
    let interp_ptr = ACTIVE_INTERP.with(Cell::get);
    if interp_ptr.is_null() {
        return AbiVal::Int(0);
    }
    // SAFETY: `ACTIVE_INTERP` is exclusively set by `ActiveInterpGuard` on this exact
    // thread for the duration of the enclosing `invoke_c_function` call.
    let interp = unsafe { &mut *interp_ptr };

    let mut py_args = Vec::with_capacity(slot.argtypes.len());
    for (i, arg_type) in slot.argtypes.iter().enumerate() {
        let raw = raw_args.get(i).copied().unwrap_or(AbiVal::Int(0));
        let py_val = match super::abi_val_to_python(interp, arg_type, raw) {
            Ok(v) => v,
            Err(_) => return AbiVal::Int(0),
        };
        py_args.push(ExpandedCallArg {
            name: None,
            value: py_val,
        });
    }

    let ret_val = match interp.call_function_expanded(slot.callable, &py_args) {
        Ok(v) => v,
        Err(_) => return AbiVal::Int(0),
    };

    match slot.restype {
        None => AbiVal::Int(0),
        Some(ref rt) => super::python_to_abi_return(interp, rt, &ret_val).unwrap_or(AbiVal::Int(0)),
    }
}

macro_rules! define_int4_ret_int_trampolines {
    ($($idx:literal),* $(,)?) => {
        static INT4_RET_INT_FNS: [extern "C" fn(u64, u64, u64, u64) -> u64; INT4_RET_INT_COUNT] = [
            $({
                extern "C" fn tramp(a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
                    run_callback(
                        CallbackPoolKind::Int4ToInt,
                        $idx,
                        &[
                            AbiVal::Int(a0),
                            AbiVal::Int(a1),
                            AbiVal::Int(a2),
                            AbiVal::Int(a3),
                        ],
                    )
                    .as_int_bits()
                }
                tramp
            }),*
        ];
    };
}

define_int4_ret_int_trampolines!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31
);

macro_rules! define_int8_ret_int_trampolines {
    ($($idx:literal),* $(,)?) => {
        static INT8_RET_INT_FNS: [extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64; INT8_RET_INT_COUNT] = [
            $({
                extern "C" fn tramp(
                    a0: u64,
                    a1: u64,
                    a2: u64,
                    a3: u64,
                    a4: u64,
                    a5: u64,
                    a6: u64,
                    a7: u64,
                ) -> u64 {
                    run_callback(
                        CallbackPoolKind::Int8ToInt,
                        $idx,
                        &[
                            AbiVal::Int(a0),
                            AbiVal::Int(a1),
                            AbiVal::Int(a2),
                            AbiVal::Int(a3),
                            AbiVal::Int(a4),
                            AbiVal::Int(a5),
                            AbiVal::Int(a6),
                            AbiVal::Int(a7),
                        ],
                    )
                    .as_int_bits()
                }
                tramp
            }),*
        ];
    };
}

define_int8_ret_int_trampolines!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);

macro_rules! define_int4_ret_f64_trampolines {
    ($($idx:literal),* $(,)?) => {
        static INT4_RET_F64_FNS: [extern "C" fn(u64, u64, u64, u64) -> f64; INT4_RET_F64_COUNT] = [
            $({
                extern "C" fn tramp(a0: u64, a1: u64, a2: u64, a3: u64) -> f64 {
                    run_callback(
                        CallbackPoolKind::Int4ToF64,
                        $idx,
                        &[
                            AbiVal::Int(a0),
                            AbiVal::Int(a1),
                            AbiVal::Int(a2),
                            AbiVal::Int(a3),
                        ],
                    )
                    .as_f64()
                }
                tramp
            }),*
        ];
    };
}

define_int4_ret_f64_trampolines!(0, 1, 2, 3, 4, 5, 6, 7);

macro_rules! define_int4_ret_f32_trampolines {
    ($($idx:literal),* $(,)?) => {
        static INT4_RET_F32_FNS: [extern "C" fn(u64, u64, u64, u64) -> f32; INT4_RET_F32_COUNT] = [
            $({
                extern "C" fn tramp(a0: u64, a1: u64, a2: u64, a3: u64) -> f32 {
                    run_callback(
                        CallbackPoolKind::Int4ToF32,
                        $idx,
                        &[
                            AbiVal::Int(a0),
                            AbiVal::Int(a1),
                            AbiVal::Int(a2),
                            AbiVal::Int(a3),
                        ],
                    )
                    .as_f32()
                }
                tramp
            }),*
        ];
    };
}

define_int4_ret_f32_trampolines!(0, 1, 2, 3, 4, 5, 6, 7);

macro_rules! define_f64_ret_f64_trampolines {
    ($($idx:literal),* $(,)?) => {
        static F64_RET_F64_FNS: [extern "C" fn(f64, f64, f64, f64) -> f64; F64_RET_F64_COUNT] = [
            $({
                extern "C" fn tramp(a0: f64, a1: f64, a2: f64, a3: f64) -> f64 {
                    run_callback(
                        CallbackPoolKind::F64ToF64,
                        $idx,
                        &[
                            AbiVal::F64(a0),
                            AbiVal::F64(a1),
                            AbiVal::F64(a2),
                            AbiVal::F64(a3),
                        ],
                    )
                    .as_f64()
                }
                tramp
            }),*
        ];
    };
}

define_f64_ret_f64_trampolines!(0, 1, 2, 3, 4, 5, 6, 7);

macro_rules! define_f64_ret_int_trampolines {
    ($($idx:literal),* $(,)?) => {
        static F64_RET_INT_FNS: [extern "C" fn(f64, f64, f64, f64) -> u64; F64_RET_INT_COUNT] = [
            $({
                extern "C" fn tramp(a0: f64, a1: f64, a2: f64, a3: f64) -> u64 {
                    run_callback(
                        CallbackPoolKind::F64ToInt,
                        $idx,
                        &[
                            AbiVal::F64(a0),
                            AbiVal::F64(a1),
                            AbiVal::F64(a2),
                            AbiVal::F64(a3),
                        ],
                    )
                    .as_int_bits()
                }
                tramp
            }),*
        ];
    };
}

define_f64_ret_int_trampolines!(0, 1, 2, 3, 4, 5, 6, 7);

macro_rules! define_f32_ret_f32_trampolines {
    ($($idx:literal),* $(,)?) => {
        static F32_RET_F32_FNS: [extern "C" fn(f32, f32, f32, f32) -> f32; F32_RET_F32_COUNT] = [
            $({
                extern "C" fn tramp(a0: f32, a1: f32, a2: f32, a3: f32) -> f32 {
                    run_callback(
                        CallbackPoolKind::F32ToF32,
                        $idx,
                        &[
                            AbiVal::F32(a0),
                            AbiVal::F32(a1),
                            AbiVal::F32(a2),
                            AbiVal::F32(a3),
                        ],
                    )
                    .as_f32()
                }
                tramp
            }),*
        ];
    };
}

define_f32_ret_f32_trampolines!(0, 1, 2, 3, 4, 5, 6, 7);

fn classify_type_abi(val: &Value) -> AbiKind {
    if let ValueKind::PyClass(cls) = val.kind()
        && let Some(CTypeInfo::Simple { kind, .. }) = super::types::lookup_ctype(cls)
    {
        return match kind {
            PrimKind::Float => AbiKind::F32,
            PrimKind::Double | PrimKind::LongDouble => AbiKind::F64,
            _ => AbiKind::Int,
        };
    }
    AbiKind::Int
}

/// Allocate a static C-callable trampoline pointer for a Python callable.
pub(crate) fn allocate_callback(
    callable: Value,
    restype: Option<Value>,
    argtypes: Vec<Value>,
) -> Result<(usize, CallbackSlotHandle)> {
    let ret_abi = restype.as_ref().map_or(AbiKind::Int, classify_type_abi);
    let arg_abis: Vec<AbiKind> = argtypes.iter().map(classify_type_abi).collect();
    let all_int = arg_abis.iter().all(|k| *k == AbiKind::Int);
    let all_f64 = !arg_abis.is_empty() && arg_abis.iter().all(|k| *k == AbiKind::F64);
    let all_f32 = !arg_abis.is_empty() && arg_abis.iter().all(|k| *k == AbiKind::F32);

    let pool = if all_int && arg_abis.len() <= 4 {
        match ret_abi {
            AbiKind::Int => CallbackPoolKind::Int4ToInt,
            AbiKind::F64 => CallbackPoolKind::Int4ToF64,
            AbiKind::F32 => CallbackPoolKind::Int4ToF32,
        }
    } else if all_int && arg_abis.len() <= 8 && ret_abi == AbiKind::Int {
        CallbackPoolKind::Int8ToInt
    } else if all_f64 && arg_abis.len() <= 4 {
        match ret_abi {
            AbiKind::F64 => CallbackPoolKind::F64ToF64,
            AbiKind::Int => CallbackPoolKind::F64ToInt,
            AbiKind::F32 => {
                return Err(PyError::named(
                    "TypeError",
                    "unsupported CFUNCTYPE callback signature (f64 args -> f32)",
                ));
            }
        }
    } else if all_f32 && arg_abis.len() <= 4 && ret_abi == AbiKind::F32 {
        CallbackPoolKind::F32ToF32
    } else {
        return Err(PyError::named(
            "TypeError",
            "unsupported CFUNCTYPE callback signature",
        ));
    };

    let slot = CallbackSlot {
        callable,
        restype,
        argtypes,
    };

    let index = CALLBACK_REGISTRY.with(|reg| {
        let mut guard = reg.borrow_mut();
        let slice = guard.slice_mut(pool);
        for (i, entry) in slice.iter_mut().enumerate() {
            if entry.is_none() {
                *entry = Some(slot);
                return Ok(i);
            }
        }
        Err(PyError::named(
            "MemoryError",
            "maximum number of active CFUNCTYPE callbacks reached",
        ))
    })?;

    let fn_addr = match pool {
        CallbackPoolKind::Int4ToInt => INT4_RET_INT_FNS[index] as usize,
        CallbackPoolKind::Int8ToInt => INT8_RET_INT_FNS[index] as usize,
        CallbackPoolKind::Int4ToF64 => INT4_RET_F64_FNS[index] as usize,
        CallbackPoolKind::Int4ToF32 => INT4_RET_F32_FNS[index] as usize,
        CallbackPoolKind::F64ToF64 => F64_RET_F64_FNS[index] as usize,
        CallbackPoolKind::F64ToInt => F64_RET_INT_FNS[index] as usize,
        CallbackPoolKind::F32ToF32 => F32_RET_F32_FNS[index] as usize,
    };

    Ok((fn_addr, CallbackSlotHandle { pool, index }))
}
