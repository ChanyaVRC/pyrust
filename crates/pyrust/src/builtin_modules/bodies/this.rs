// `this` module — 100% native Rust implementation of Python 3.12's `this`
// module ("The Zen of Python" Easter egg, issue #3248).
//
// Exposes the exact CPython `s` (ROT-13 payload), `d` (ROT-13 translation
// dict), `c` (`97`), and `i` (`25`), and prints the decoded Zen of Python to
// `sys.stdout` on import via `prepare_builtin_module`.
//
// Reference: CPython `Lib/this.py`

use pyrust_derive::pyrust_module;

use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value};

/// Exact ROT-13 Zen of Python payload from CPython 3.12 `Lib/this.py`.
const ZEN_ROT13: &str = "Gur Mra bs Clguba, ol Gvz Crgref\n\
\n\
Ornhgvshy vf orggre guna htyl.\n\
Rkcyvpvg vf orggre guna vzcyvpvg.\n\
Fvzcyr vf orggre guna pbzcyrk.\n\
Pbzcyrk vf orggre guna pbzcyvpngrq.\n\
Syng vf orggre guna arfgrq.\n\
Fcnefr vf orggre guna qrafr.\n\
Ernqnovyvgl pbhagf.\n\
Fcrpvny pnfrf nera'g fcrpvny rabhtu gb oernx gur ehyrf.\n\
Nygubhtu cenpgvpnyvgl orngf chevgl.\n\
Reebef fubhyq arire cnff fvyragyl.\n\
Hayrff rkcyvpvgyl fvyraprq.\n\
Va gur snpr bs nzovthvgl, ershfr gur grzcgngvba gb thrff.\n\
Gurer fubhyq or bar-- naq cersrenoyl bayl bar --boivbhf jnl gb qb vg.\n\
Nygubhtu gung jnl znl abg or boivbhf ng svefg hayrff lbh'er Qhgpu.\n\
Abj vf orggre guna arire.\n\
Nygubhtu arire vf bsgra orggre guna *evtug* abj.\n\
Vs gur vzcyrzragngvba vf uneq gb rkcynva, vg'f n onq vqrn.\n\
Vs gur vzcyrzragngvba vf rnfl gb rkcynva, vg znl or n tbbq vqrn.\n\
Anzrfcnprf ner bar ubaxvat terng vqrn -- yrg'f qb zber bs gubfr!";

fn build_rot13_dict() -> Value {
    let mut dict = PyDict::default();
    for base in [65u8, 97u8] {
        for offset in 0u8..26u8 {
            let src = (base + offset) as char;
            let dst = (base + ((offset + 13) % 26)) as char;
            let mut src_buf = [0u8; 4];
            let mut dst_buf = [0u8; 4];
            dict.insert(
                PyKey::str_from(src.encode_utf8(&mut src_buf)),
                Value::string(dst.encode_utf8(&mut dst_buf)),
            );
        }
    }
    Value::dict(dict)
}

fn decode_rot13(s: &str) -> String {
    s.chars()
        .map(|ch| match ch {
            'A'..='Z' => (b'A' + ((ch as u8 - b'A' + 13) % 26)) as char,
            'a'..='z' => (b'a' + ((ch as u8 - b'a' + 13) % 26)) as char,
            other => other,
        })
        .collect()
}

/// Print the decoded Zen of Python to `sys.stdout` on module import.
pub(crate) fn prepare_module(interp: &mut Interpreter) {
    let mut decoded = decode_rot13(ZEN_ROT13);
    decoded.push('\n');
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(stdout) = interp.get_attr(&sys_mod, "stdout")
        && let Ok(write_fn) = interp.get_attr(&stdout, "write")
    {
        let _ = interp.call_function_expanded(
            write_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(decoded),
            }],
        );
    }
}

pyrust_module! {
    constants {
        "s" => Value::string(ZEN_ROT13),
        "d" => build_rot13_dict(),
        "c" => Value::int(97),
        "i" => Value::int(25),
    }
}
