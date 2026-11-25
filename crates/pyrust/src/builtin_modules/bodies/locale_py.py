"""Locale support module (CPython 3.12 parity, issue #3226)."""

import os
import re
import sys

__all__ = [
    "getlocale",
    "getdefaultlocale",
    "getpreferredencoding",
    "Error",
    "setlocale",
    "resetlocale",
    "localeconv",
    "strcoll",
    "strxfrm",
    "str",
    "atof",
    "atoi",
    "format_string",
    "currency",
    "normalize",
    "LC_CTYPE",
    "LC_COLLATE",
    "LC_TIME",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_ALL",
    "CHAR_MAX",
    "getencoding",
]

_builtin_str = str


class Error(Exception):
    """Exception raised for locale errors."""


# ---------------------------------------------------------------------------
# Encoding and locale alias tables (subset aligned with CPython 3.12 Lib/locale.py)
# ---------------------------------------------------------------------------

encoding_alias = {
    "437": "C",
    "c": "C",
    "en": "ISO8859-1",
    "jis": "JIS7",
    "jis7": "JIS7",
    "ajec": "eucJP",
    "koi8c": "KOI8-C",
    "microsoftcp1251": "CP1251",
    "microsoftcp1255": "CP1255",
    "microsoftcp1256": "CP1256",
    "88591": "ISO8859-1",
    "88592": "ISO8859-2",
    "88595": "ISO8859-5",
    "885915": "ISO8859-15",
    "ascii": "ISO8859-1",
    "latin_1": "ISO8859-1",
    "iso8859_1": "ISO8859-1",
    "iso8859_2": "ISO8859-2",
    "iso8859_3": "ISO8859-3",
    "iso8859_4": "ISO8859-4",
    "iso8859_5": "ISO8859-5",
    "iso8859_6": "ISO8859-6",
    "iso8859_7": "ISO8859-7",
    "iso8859_8": "ISO8859-8",
    "iso8859_9": "ISO8859-9",
    "iso8859_10": "ISO8859-10",
    "iso8859_11": "ISO8859-11",
    "iso8859_13": "ISO8859-13",
    "iso8859_14": "ISO8859-14",
    "iso8859_15": "ISO8859-15",
    "iso8859_16": "ISO8859-16",
    "iso8859-1": "ISO8859-1",
    "iso8859-2": "ISO8859-2",
    "iso8859-5": "ISO8859-5",
    "iso8859-15": "ISO8859-15",
    "iso88591": "ISO8859-1",
    "iso88592": "ISO8859-2",
    "iso88595": "ISO8859-5",
    "iso885915": "ISO8859-15",
    "iso_8859_1": "ISO8859-1",
    "iso_8859_15": "ISO8859-15",
    "latin1": "ISO8859-1",
    "latin9": "ISO8859-15",
    "utf_8": "UTF-8",
    "utf-8": "UTF-8",
    "utf8": "UTF-8",
    "utf8@euro": "UTF-8",
    "cp1250": "CP1250",
    "cp1251": "CP1251",
    "cp1252": "CP1252",
    "cp1253": "CP1253",
    "cp1254": "CP1254",
    "cp1255": "CP1255",
    "cp1256": "CP1256",
    "cp1257": "CP1257",
    "cp1258": "CP1258",
    "cp932": "CP932",
    "euc_jp": "eucJP",
    "eucjp": "eucJP",
    "euc_kr": "eucKR",
    "euckr": "eucKR",
    "gb2312": "gb2312",
    "gbk": "GBK",
    "gb18030": "GB18030",
    "big5": "Big5",
    "koi8_r": "KOI8-R",
    "koi8r": "KOI8-R",
    "koi8_u": "KOI8-U",
    "shift_jis": "SJIS",
    "sjis": "SJIS",
}

locale_alias = {
    "c": "C",
    "posix": "C",
    "c.utf8": "C.UTF-8",
    "c.utf-8": "C.UTF-8",
    "ar": "ar_AA.ISO8859-6",
    "ar_eg": "ar_EG.ISO8859-6",
    "ar_sa": "ar_SA.ISO8859-6",
    "bg": "bg_BG.ISO8859-5",
    "bg_bg": "bg_BG.ISO8859-5",
    "ca": "ca_ES.ISO8859-1",
    "ca_es": "ca_ES.ISO8859-1",
    "cs": "cs_CZ.ISO8859-2",
    "cs_cz": "cs_CZ.ISO8859-2",
    "da": "da_DK.ISO8859-1",
    "da_dk": "da_DK.ISO8859-1",
    "de": "de_DE.ISO8859-1",
    "de_at": "de_AT.ISO8859-1",
    "de_ch": "de_CH.ISO8859-1",
    "de_de": "de_DE.ISO8859-1",
    "de_de@euro": "de_DE.ISO8859-15",
    "el": "el_GR.ISO8859-7",
    "el_gr": "el_GR.ISO8859-7",
    "en": "en_US.ISO8859-1",
    "en_au": "en_AU.ISO8859-1",
    "en_ca": "en_CA.ISO8859-1",
    "en_gb": "en_GB.ISO8859-1",
    "en_ie": "en_IE.ISO8859-1",
    "en_nz": "en_NZ.ISO8859-1",
    "en_us": "en_US.ISO8859-1",
    "en_us@euro": "en_US.ISO8859-15",
    "es": "es_ES.ISO8859-1",
    "es_ar": "es_AR.ISO8859-1",
    "es_es": "es_ES.ISO8859-1",
    "es_es@euro": "es_ES.ISO8859-15",
    "es_mx": "es_MX.ISO8859-1",
    "fi": "fi_FI.ISO8859-1",
    "fi_fi": "fi_FI.ISO8859-1",
    "fr": "fr_FR.ISO8859-1",
    "fr_be": "fr_BE.ISO8859-1",
    "fr_ca": "fr_CA.ISO8859-1",
    "fr_ch": "fr_CH.ISO8859-1",
    "fr_fr": "fr_FR.ISO8859-1",
    "fr_fr@euro": "fr_FR.ISO8859-15",
    "he": "he_IL.ISO8859-8",
    "he_il": "he_IL.ISO8859-8",
    "hi": "hi_IN.ISCII-DEV",
    "hi_in": "hi_IN.ISCII-DEV",
    "hr": "hr_HR.ISO8859-2",
    "hr_hr": "hr_HR.ISO8859-2",
    "hu": "hu_HU.ISO8859-2",
    "hu_hu": "hu_HU.ISO8859-2",
    "id": "id_ID.ISO8859-1",
    "id_id": "id_ID.ISO8859-1",
    "it": "it_IT.ISO8859-1",
    "it_ch": "it_CH.ISO8859-1",
    "it_it": "it_IT.ISO8859-1",
    "it_it@euro": "it_IT.ISO8859-15",
    "ja": "ja_JP.eucJP",
    "ja_jp": "ja_JP.eucJP",
    "ja_jp.sjis": "ja_JP.SJIS",
    "ja_jp.euc": "ja_JP.eucJP",
    "ko": "ko_KR.eucKR",
    "ko_kr": "ko_KR.eucKR",
    "nb": "nb_NO.ISO8859-1",
    "nb_no": "nb_NO.ISO8859-1",
    "nl": "nl_NL.ISO8859-1",
    "nl_be": "nl_BE.ISO8859-1",
    "nl_nl": "nl_NL.ISO8859-1",
    "no": "no_NO.ISO8859-1",
    "no_no": "no_NO.ISO8859-1",
    "pl": "pl_PL.ISO8859-2",
    "pl_pl": "pl_PL.ISO8859-2",
    "pt": "pt_PT.ISO8859-1",
    "pt_br": "pt_BR.ISO8859-1",
    "pt_pt": "pt_PT.ISO8859-1",
    "ro": "ro_RO.ISO8859-2",
    "ro_ro": "ro_RO.ISO8859-2",
    "ru": "ru_RU.ISO8859-5",
    "ru_ru": "ru_RU.ISO8859-5",
    "sk": "sk_SK.ISO8859-2",
    "sk_sk": "sk_SK.ISO8859-2",
    "sv": "sv_SE.ISO8859-1",
    "sv_se": "sv_SE.ISO8859-1",
    "th": "th_TH.ISO8859-11",
    "th_th": "th_TH.ISO8859-11",
    "tr": "tr_TR.ISO8859-9",
    "tr_tr": "tr_TR.ISO8859-9",
    "uk": "uk_UA.KOI8-U",
    "uk_ua": "uk_UA.KOI8-U",
    "vi": "vi_VN.TCVN",
    "vi_vn": "vi_VN.TCVN",
    "zh": "zh_CN.eucCN",
    "zh_cn": "zh_CN.gb2312",
    "zh_hk": "zh_HK.big5hkscs",
    "zh_tw": "zh_TW.big5",
    "english": "en_EN.ISO8859-1",
    "french": "fr_FR.ISO8859-1",
    "german": "de_DE.ISO8859-1",
    "japanese": "ja_JP.eucJP",
    "korean": "ko_KR.eucKR",
    "spanish": "es_ES.ISO8859-1",
    "italian": "it_IT.ISO8859-1",
    "portuguese": "pt_PT.ISO8859-1",
    "russian": "ru_RU.ISO8859-5",
    "chinese": "zh_CN.gb2312",
}

windows_locale = {
    0x0409: "en_US",
    0x0809: "en_GB",
    0x0C09: "en_AU",
    0x1009: "en_CA",
    0x0407: "de_DE",
    0x040C: "fr_FR",
    0x0C0A: "es_ES",
    0x0410: "it_IT",
    0x0411: "ja_JP",
    0x0412: "ko_KR",
    0x0804: "zh_CN",
    0x0404: "zh_TW",
    0x0416: "pt_BR",
    0x0816: "pt_PT",
    0x0419: "ru_RU",
    0x0413: "nl_NL",
    0x0415: "pl_PL",
    0x041D: "sv_SE",
}


# ---------------------------------------------------------------------------
# Locale normalization (CPython 3.12 algorithm)
# ---------------------------------------------------------------------------

def _replace_encoding(code, encoding):
    if "." in code:
        langname = code[: code.index(".")]
    else:
        langname = code
    norm_encoding = encoding.lower()
    norm_encoding = encoding_alias.get(norm_encoding, norm_encoding)
    if norm_encoding == encoding.lower():
        norm_encoding = encoding_alias.get(
            norm_encoding.replace("-", "_"), encoding
        )
    return langname + "." + norm_encoding


def _append_modifier(code, modifier):
    if modifier == "euro":
        if "." not in code:
            return code + ".ISO8859-15"
        _, _, encoding = code.partition(".")
        if encoding in ("ISO8859-15", "UTF-8"):
            return code
        if encoding == "ISO8859-1":
            return _replace_encoding(code, "ISO8859-15")
    return code + "@" + modifier


def normalize(localename):
    """Return a normalized locale code for the given locale name."""
    code = localename.lower()
    if ":" in code:
        code = code.replace(":", ".")
    if "@" in code:
        code, modifier = code.split("@", 1)
    else:
        modifier = ""
    if "." in code:
        langname, encoding = code.split(".", 1)
    else:
        langname = code
        encoding = ""

    lookup_name = langname.replace("-", "_")
    if encoding:
        lookup_enc = encoding.replace("-", "_")
        lookup_enc = encoding_alias.get(lookup_enc, lookup_enc)
        if modifier:
            fullname = f"{lookup_name}.{lookup_enc}@{modifier}"
            if fullname in locale_alias:
                return locale_alias[fullname]
        fullname = f"{lookup_name}.{lookup_enc}"
        if fullname in locale_alias:
            code = locale_alias[fullname]
            if modifier:
                return _append_modifier(code, modifier)
            return code

    if modifier:
        fullname = f"{lookup_name}@{modifier}"
        if fullname in locale_alias:
            code = locale_alias[fullname]
            if encoding:
                return _replace_encoding(code, encoding)
            return code

    if lookup_name in locale_alias:
        code = locale_alias[lookup_name]
        if encoding:
            code = _replace_encoding(code, encoding)
        if modifier:
            code = _append_modifier(code, modifier)
        return code

    if encoding:
        # Unknown language, but still normalize known encoding if applicable
        if encoding.lower() in encoding_alias or encoding.lower().replace("-", "_") in encoding_alias:
            base = localename.split("@", 1)[0].split(".", 1)[0]
            res = _replace_encoding(base, encoding)
            if modifier:
                res = _append_modifier(res, modifier)
            return res

    return localename


# ---------------------------------------------------------------------------
# Locale state & localeconv presets
# ---------------------------------------------------------------------------

_C_LOCALECONV = {
    "int_curr_symbol": "",
    "currency_symbol": "",
    "mon_decimal_point": "",
    "mon_thousands_sep": "",
    "mon_grouping": [],
    "positive_sign": "",
    "negative_sign": "",
    "int_frac_digits": CHAR_MAX,
    "frac_digits": CHAR_MAX,
    "p_cs_precedes": CHAR_MAX,
    "p_sep_by_space": CHAR_MAX,
    "n_cs_precedes": CHAR_MAX,
    "n_sep_by_space": CHAR_MAX,
    "p_sign_posn": CHAR_MAX,
    "n_sign_posn": CHAR_MAX,
    "decimal_point": ".",
    "thousands_sep": "",
    "grouping": [],
}

_PRESET_LOCALECONV = {
    "en_US": {
        "int_curr_symbol": "USD ",
        "currency_symbol": "$",
        "mon_decimal_point": ".",
        "mon_thousands_sep": ",",
        "mon_grouping": [3, 3, 0],
        "positive_sign": "",
        "negative_sign": "-",
        "int_frac_digits": 2,
        "frac_digits": 2,
        "p_cs_precedes": 1,
        "p_sep_by_space": 0,
        "n_cs_precedes": 1,
        "n_sep_by_space": 0,
        "p_sign_posn": 1,
        "n_sign_posn": 1,
        "decimal_point": ".",
        "thousands_sep": ",",
        "grouping": [3, 3, 0],
    },
    "en_GB": {
        "int_curr_symbol": "GBP ",
        "currency_symbol": "\u00a3",
        "mon_decimal_point": ".",
        "mon_thousands_sep": ",",
        "mon_grouping": [3, 3, 0],
        "positive_sign": "",
        "negative_sign": "-",
        "int_frac_digits": 2,
        "frac_digits": 2,
        "p_cs_precedes": 1,
        "p_sep_by_space": 0,
        "n_cs_precedes": 1,
        "n_sep_by_space": 0,
        "p_sign_posn": 1,
        "n_sign_posn": 1,
        "decimal_point": ".",
        "thousands_sep": ",",
        "grouping": [3, 3, 0],
    },
    "de_DE": {
        "int_curr_symbol": "EUR ",
        "currency_symbol": "\u20ac",
        "mon_decimal_point": ",",
        "mon_thousands_sep": ".",
        "mon_grouping": [3, 3, 0],
        "positive_sign": "",
        "negative_sign": "-",
        "int_frac_digits": 2,
        "frac_digits": 2,
        "p_cs_precedes": 0,
        "p_sep_by_space": 1,
        "n_cs_precedes": 0,
        "n_sep_by_space": 1,
        "p_sign_posn": 1,
        "n_sign_posn": 1,
        "decimal_point": ",",
        "thousands_sep": ".",
        "grouping": [3, 3, 0],
    },
    "fr_FR": {
        "int_curr_symbol": "EUR ",
        "currency_symbol": "\u20ac",
        "mon_decimal_point": ",",
        "mon_thousands_sep": " ",
        "mon_grouping": [3, 0],
        "positive_sign": "",
        "negative_sign": "-",
        "int_frac_digits": 2,
        "frac_digits": 2,
        "p_cs_precedes": 0,
        "p_sep_by_space": 1,
        "n_cs_precedes": 0,
        "n_sep_by_space": 1,
        "p_sign_posn": 1,
        "n_sign_posn": 1,
        "decimal_point": ",",
        "thousands_sep": " ",
        "grouping": [3, 0],
    },
    "ja_JP": {
        "int_curr_symbol": "JPY ",
        "currency_symbol": "\uffe5",
        "mon_decimal_point": ".",
        "mon_thousands_sep": ",",
        "mon_grouping": [3, 0],
        "positive_sign": "",
        "negative_sign": "-",
        "int_frac_digits": 0,
        "frac_digits": 0,
        "p_cs_precedes": 1,
        "p_sep_by_space": 0,
        "n_cs_precedes": 1,
        "n_sep_by_space": 0,
        "p_sign_posn": 1,
        "n_sign_posn": 4,
        "decimal_point": ".",
        "thousands_sep": ",",
        "grouping": [3, 0],
    },
}

_SUPPORTED_LOCALE_LANGS = set(_PRESET_LOCALECONV.keys()) | {
    "C",
    "POSIX",
    "en_AU",
    "en_CA",
    "es_ES",
    "it_IT",
    "pt_BR",
    "pt_PT",
    "ru_RU",
    "zh_CN",
    "zh_TW",
    "ko_KR",
    "nl_NL",
    "pl_PL",
    "sv_SE",
}

_CATEGORY_NAMES = {
    LC_CTYPE: "LC_CTYPE",
    LC_NUMERIC: "LC_NUMERIC",
    LC_TIME: "LC_TIME",
    LC_COLLATE: "LC_COLLATE",
    LC_MONETARY: "LC_MONETARY",
}
if "LC_MESSAGES" in globals():
    _CATEGORY_NAMES[LC_MESSAGES] = "LC_MESSAGES"

_VALID_CATEGORIES = set(_CATEGORY_NAMES.keys()) | {LC_ALL}

# Process-local category state (matching CPython startup: LC_NUMERIC/etc. = "C",
# LC_CTYPE initialized from environment or C.UTF-8).
def _env_default_locale(category_name=None):
    for var in ("LC_ALL", category_name, "LANG"):
        if not var:
            continue
        val = os.environ.get(var)
        if val:
            return val
    return "C"


_state = {cat: "C" for cat in _CATEGORY_NAMES}
_init_ctype = _env_default_locale("LC_CTYPE")
if _init_ctype and _init_ctype not in ("C", "POSIX"):
    _state[LC_CTYPE] = _init_ctype
else:
    _state[LC_CTYPE] = "C.UTF-8" if sys.platform != "win32" else "C"

_override_localeconv = {}


def _parse_localename(localename):
    """Parse a locale string into a (language_code, encoding) 2-tuple."""
    code = normalize(localename)
    if "@" in code:
        code, modifier = code.split("@", 1)
        if modifier == "euro" and "." not in code:
            return code, "iso-8859-15"
    if "." in code:
        return tuple(code.split(".")[:2])
    if code == "C":
        return None, None
    if code == "UTF-8":
        return None, "UTF-8"
    raise ValueError(f"unknown locale: {localename}")


def _build_localename(localetuple):
    """Build a locale string from a (language, encoding) 2-tuple."""
    try:
        language, encoding = localetuple
        if language is None:
            language = "C"
        if encoding is None:
            return language
        return language + "." + encoding
    except (TypeError, ValueError):
        raise TypeError("Locale must be None, a string, or an iterable of two strings -- language code, encoding.")


def _validate_and_canonicalize_locale(loc_str, category_name=None):
    if loc_str == "":
        env_val = _env_default_locale(category_name)
        if env_val in ("", "C", "POSIX"):
            return "C"
        loc_str = env_val

    if loc_str in ("C", "POSIX"):
        return "C"

    upper = loc_str.upper()
    if upper in ("C.UTF-8", "C.UTF8", "POSIX.UTF-8", "POSIX.UTF8"):
        return "C.UTF-8"

    norm = normalize(loc_str)
    if norm == "C":
        return "C"
    base = norm.split("@", 1)[0].split(".", 1)[0]
    if base in _SUPPORTED_LOCALE_LANGS:
        return loc_str

    raise Error(f"unsupported locale setting")


def setlocale(category, locale=None):
    """Set the locale for the given category, or query it when locale is None."""
    if not isinstance(category, int) or isinstance(category, bool) or category not in _VALID_CATEGORIES:
        raise Error("invalid locale category")

    if locale is None:
        if category == LC_ALL:
            vals = list(_state.values())
            if len(set(vals)) == 1:
                return vals[0]
            return ";".join(
                f"{_CATEGORY_NAMES[c]}={_state[c]}" for c in _CATEGORY_NAMES
            )
        return _state[category]

    if not isinstance(locale, _builtin_str):
        locale = _build_localename(locale)

    if category == LC_ALL:
        # Handle composite LC_ALL string from previous setlocale(LC_ALL)
        if ";" in locale and "=" in locale:
            parts = {}
            for seg in locale.split(";"):
                if "=" not in seg:
                    raise Error("unsupported locale setting")
                k, v = seg.split("=", 1)
                parts[k] = v
            new_vals = {}
            for cat, cname in _CATEGORY_NAMES.items():
                if cname in parts:
                    new_vals[cat] = _validate_and_canonicalize_locale(
                        parts[cname], cname
                    )
            for cat, val in new_vals.items():
                _state[cat] = val
            return setlocale(LC_ALL, None)
        canon = _validate_and_canonicalize_locale(locale, None)
        for cat in _CATEGORY_NAMES:
            _state[cat] = canon
        return canon

    cname = _CATEGORY_NAMES[category]
    canon = _validate_and_canonicalize_locale(locale, cname)
    _state[category] = canon
    return canon


def getlocale(category=LC_CTYPE):
    """Return the current setting for the given locale category as (lang, enc)."""
    localename = setlocale(category)
    if category == LC_ALL and ";" in localename:
        raise TypeError("category LC_ALL is not supported")
    return _parse_localename(localename)


def getdefaultlocale(envvars=("LC_ALL", "LC_CTYPE", "LANG", "LANGUAGE")):
    """Try to determine the default locale settings and return (lang, enc)."""
    localename = None
    for variable in envvars:
        val = os.environ.get(variable)
        if val:
            if variable == "LANGUAGE":
                val = val.split(":")[0]
            localename = val
            break
    if not localename:
        return None, None
    return _parse_localename(localename)


def resetlocale(category=LC_ALL):
    """Set the locale for category to the default setting."""
    setlocale(category, _build_localename(getdefaultlocale()))


def getencoding():
    """Return the current locale encoding."""
    ctype = _state[LC_CTYPE]
    if "." in ctype:
        enc = ctype.split(".", 1)[1].split("@", 1)[0]
        if enc:
            return enc
    return "utf-8" if sys.platform == "win32" else "UTF-8"


def getpreferredencoding(do_setlocale=True):
    """Return the encoding used for text data, according to user preferences."""
    return "utf-8"


def localeconv():
    """Return the database of local conventions as a dictionary."""
    num_loc = _state[LC_NUMERIC]
    mon_loc = _state[LC_MONETARY]

    res = dict(_C_LOCALECONV)
    res["grouping"] = list(res["grouping"])
    res["mon_grouping"] = list(res["mon_grouping"])

    if num_loc not in ("C", "POSIX", "C.UTF-8"):
        norm = normalize(num_loc).split("@", 1)[0].split(".", 1)[0]
        preset = _PRESET_LOCALECONV.get(norm)
        if preset is not None:
            res["decimal_point"] = preset["decimal_point"]
            res["thousands_sep"] = preset["thousands_sep"]
            res["grouping"] = list(preset["grouping"])

    if mon_loc not in ("C", "POSIX", "C.UTF-8"):
        norm = normalize(mon_loc).split("@", 1)[0].split(".", 1)[0]
        preset = _PRESET_LOCALECONV.get(norm)
        if preset is not None:
            for k in preset:
                if k not in ("decimal_point", "thousands_sep", "grouping"):
                    val = preset[k]
                    res[k] = list(val) if isinstance(val, list) else val

    mod = sys.modules.get("locale")
    overrides = (
        getattr(mod, "_override_localeconv", _override_localeconv)
        if mod is not None
        else _override_localeconv
    )
    if overrides:
        res.update(overrides)
    return res


# ---------------------------------------------------------------------------
# Grouping, formatting, currency, localize/delocalize, atof/atoi, collation
# ---------------------------------------------------------------------------

def _grouping_intervals(grouping):
    last_interval = None
    for interval in grouping:
        if interval == CHAR_MAX:
            return
        if interval == 0:
            if last_interval is None:
                raise ValueError("invalid grouping")
            while True:
                yield last_interval
        yield interval
        last_interval = interval


def _group(s, monetary=False):
    conv = localeconv()
    thousands_sep = conv[monetary and "mon_thousands_sep" or "thousands_sep"]
    grouping = conv[monetary and "mon_grouping" or "grouping"]
    if not grouping:
        return (s, 0)
    if s[-1] not in "0123456789":
        stripped = s.rstrip()
        right_spaces = s[len(stripped) :]
        s = stripped
    else:
        right_spaces = ""
    left_spaces = ""
    groups = []
    for interval in _grouping_intervals(grouping):
        if not s or s[-1] not in "0123456789":
            left_spaces = s
            s = ""
            break
        groups.append(s[-interval:])
        s = s[:-interval]
    if s:
        groups.append(s)
    groups.reverse()
    return (
        left_spaces + thousands_sep.join(groups) + right_spaces,
        len(thousands_sep) * (len(groups) - 1),
    )


def _strip_padding(s, amount):
    lpos = 0
    while amount and s[lpos] == " ":
        lpos += 1
        amount -= 1
    rpos = len(s) - 1
    while amount and s[rpos] == " ":
        rpos -= 1
        amount -= 1
    return s[lpos : rpos + 1]


_percent_re = re.compile(
    r"%(?:\((?P<key>.*?)\))?"
    r"(?P<modifiers>[-#0-9 +*.hlL]*?)[eEfFgGdiouxXcrs%]"
)


def _format(percent, value, grouping=False, monetary=False, *additional):
    if additional:
        formatted = percent % ((value,) + additional)
    else:
        formatted = percent % value
    if percent[-1] in "eEfFgG":
        seps = 0
        parts = formatted.split(".")
        if grouping:
            parts[0], seps = _group(parts[0], monetary=monetary)
        decimal_point = localeconv()[
            monetary and "mon_decimal_point" or "decimal_point"
        ]
        formatted = decimal_point.join(parts)
        if seps:
            formatted = _strip_padding(formatted, seps)
    elif percent[-1] in "diu":
        if grouping:
            formatted, seps = _group(formatted, monetary=monetary)
            if seps:
                formatted = _strip_padding(formatted, seps)
    return formatted


def format_string(f, val, grouping=False, monetary=False):
    """Format a string in the current locale."""
    percents = list(_percent_re.finditer(f))
    new_f = _percent_re.sub("%s", f)

    if isinstance(val, tuple):
        val = list(val)

    new_val = []
    for perc in percents:
        starcount = perc.group("modifiers").count("*")
        if perc.group() == "%%":
            new_val.append("%")
        elif perc.group("key"):
            new_val.append(_format(perc.group(), val, grouping, monetary))
        else:
            if isinstance(val, list):
                if starcount:
                    args = tuple(val[: starcount + 1])
                    del val[: starcount + 1]
                    new_val.append(
                        _format(perc.group(), args[0], grouping, monetary, *args[1:])
                    )
                else:
                    v = val.pop(0)
                    new_val.append(_format(perc.group(), v, grouping, monetary))
            else:
                new_val.append(_format(perc.group(), val, grouping, monetary))

    return new_f % tuple(new_val)


def currency(val, symbol=True, grouping=False, international=False):
    """Format `val` according to the currency settings in the current locale."""
    conv = localeconv()

    digits = conv[international and "int_frac_digits" or "frac_digits"]
    if digits == CHAR_MAX:
        raise ValueError(
            "Currency formatting is not possible using the 'C' locale."
        )

    s = localize(f"{abs(val):.{digits}f}", grouping, True)
    # '<' and '>' are markers if the sign must be inserted between symbol and value
    s = "<" + s + ">"

    if symbol:
        smb = conv[international and "int_curr_symbol" or "currency_symbol"]
        precedes = conv[val < 0 and "n_cs_precedes" or "p_cs_precedes"]
        separated = conv[val < 0 and "n_sep_by_space" or "p_sep_by_space"]

        if precedes:
            s = smb + (separated and " " or "") + s
        else:
            if international and smb[-1:] == " ":
                smb = smb[:-1]
            s = s + (separated and " " or "") + smb

    sign_pos = conv[val < 0 and "n_sign_posn" or "p_sign_posn"]
    sign = conv[val < 0 and "negative_sign" or "positive_sign"]

    if sign_pos == 0:
        s = "(" + s + ")"
    elif sign_pos == 1:
        s = sign + s
    elif sign_pos == 2:
        s = s + sign
    elif sign_pos == 3:
        s = s.replace("<", sign)
    elif sign_pos == 4:
        s = s.replace(">", sign)
    else:
        # Unknown sign_posn: default to leading sign
        s = sign + s

    return s.replace("<", "").replace(">", "")


def str(val):
    """Convert float to string, taking the locale into account."""
    return format_string("%.12g", val)


def delocalize(string):
    """Parse a string as a normalized number according to the locale settings."""
    conv = localeconv()

    ts = conv["thousands_sep"]
    if ts:
        string = string.replace(ts, "")

    dd = conv["decimal_point"]
    if dd:
        string = string.replace(dd, ".")
    return string


def localize(string, grouping=False, monetary=False):
    """Parse a string as locale number and format it according to locale settings."""
    conv = localeconv()

    # Split exponent if present
    for exp_char in ("e", "E"):
        if exp_char in string:
            mantissa, exp = string.split(exp_char, 1)
            exp = exp_char + exp
            break
    else:
        mantissa, exp = string, ""

    # Split decimal part
    if "." in mantissa:
        int_part, frac_part = mantissa.split(".", 1)
    else:
        int_part, frac_part = mantissa, None

    if grouping:
        int_part, _ = _group(int_part, monetary=monetary)

    if frac_part is not None:
        dp = conv[monetary and "mon_decimal_point" or "decimal_point"]
        return int_part + dp + frac_part + exp
    return int_part + exp


def atof(string, func=float):
    """Parse a string as a floating point number according to the locale settings."""
    return func(delocalize(string))


def atoi(string):
    """Convert a string to an integer according to the locale settings."""
    return int(delocalize(string))


def strcoll(os1, os2, /):
    """Compare two strings according to the current LC_COLLATE setting."""
    if not isinstance(os1, _builtin_str) or not isinstance(os2, _builtin_str):
        raise TypeError("strcoll() arguments must be str")
    if os1 < os2:
        return -1
    if os1 > os2:
        return 1
    return 0


def strxfrm(string, /):
    """Transform a string to one that can be used in locale-aware comparisons."""
    if not isinstance(string, _builtin_str):
        raise TypeError(
            f"strxfrm() argument must be str, not {type(string).__name__}"
        )
    return string
