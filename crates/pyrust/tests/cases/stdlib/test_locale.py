from decimal import Decimal
import locale

# 1. Constants and exception hierarchy
print("CHAR_MAX:", locale.CHAR_MAX)
print("Error is Exception:", issubclass(locale.Error, Exception))
for cname in ("LC_ALL", "LC_CTYPE", "LC_NUMERIC", "LC_TIME", "LC_COLLATE", "LC_MONETARY"):
    val = getattr(locale, cname)
    print(cname, isinstance(val, int))
print("all subset:", set(locale.__all__) >= {
    "LC_ALL", "LC_CTYPE", "LC_NUMERIC", "LC_TIME", "LC_COLLATE", "LC_MONETARY",
    "CHAR_MAX", "Error", "getlocale", "setlocale", "getpreferredencoding",
    "getencoding", "localeconv", "normalize", "format_string", "currency",
    "str", "atoi", "atof", "strcoll", "strxfrm",
})

# 2. Encoding queries return non-empty strings
enc = locale.getencoding()
pref = locale.getpreferredencoding(False)
print("encodings str:", isinstance(enc, str) and len(enc) > 0, isinstance(pref, str) and len(pref) > 0)

# 3. C locale state and error handling
saved_ctype = locale.setlocale(locale.LC_CTYPE)
try:
    print("setlocale C:", locale.setlocale(locale.LC_ALL, "C"))
    print("query LC_NUMERIC:", locale.setlocale(locale.LC_NUMERIC))
    print("getlocale LC_CTYPE:", locale.getlocale(locale.LC_CTYPE))
    print("getlocale LC_NUMERIC:", locale.getlocale(locale.LC_NUMERIC))
    print("getlocale LC_ALL:", locale.getlocale(locale.LC_ALL))

    try:
        locale.setlocale(999999, "C")
    except locale.Error:
        print("setlocale(bad_cat) -> locale.Error")

    try:
        locale.setlocale(locale.LC_ALL, "nonexistent_locale_ZZ_99999")
    except locale.Error:
        print("setlocale(bad_name) -> locale.Error")

    # Exact C localeconv dict
    conv = locale.localeconv()
    print("C localeconv:", sorted(conv.items()))

    # Currency in C locale raises ValueError
    try:
        locale.currency(12.34)
    except ValueError:
        print("currency in C -> ValueError")

    # 4. normalize()
    for name in [
        "C",
        "POSIX",
        "en_US",
        "en_US.utf8",
        "en_US.UTF-8",
        "en_US.utf8@euro",
        "fr_FR",
        "de_DE.iso88591",
        "ja_JP",
        "ja_JP.utf8",
        "pt_BR",
        "zh_CN",
        "invalid_locale_xyz",
    ]:
        print("normalize", name, "->", locale.normalize(name))

    # 5. Formatting / parsing in C locale
    print("fmt int:", locale.format_string("%d", 1234567))
    print("fmt float:", locale.format_string("%.2f", 1234.5))
    print("fmt tuple:", locale.format_string("a=%d b=%.1f%% c=%s", (42, 3.5, "ok")))
    print("fmt dict:", locale.format_string("%(x)d %(y).2f", {"x": 7, "y": 1.25}))
    print("fmt star:", locale.format_string("%*.*f", (8, 2, 3.14159)))
    print("locale.str:", locale.str(3.14), locale.str(1000.5))
    print("atof/atoi:", locale.atof("3.14"), locale.atof("2.5", Decimal), locale.atoi("42"))
    print("delocalize/localize:", locale.delocalize("1234.56"), locale.localize("1234.56"))

    # 6. Custom localeconv via _override_localeconv (European style)
    locale._override_localeconv = {
        "decimal_point": ",",
        "thousands_sep": ".",
        "grouping": [3, 0],
        "mon_decimal_point": ",",
        "mon_thousands_sep": ".",
        "mon_grouping": [3, 0],
        "currency_symbol": "EUR",
        "int_curr_symbol": "EUR ",
        "int_frac_digits": 2,
        "frac_digits": 2,
        "p_cs_precedes": 0,
        "p_sep_by_space": 1,
        "n_cs_precedes": 0,
        "n_sep_by_space": 1,
        "p_sign_posn": 1,
        "n_sign_posn": 1,
        "positive_sign": "",
        "negative_sign": "-",
    }
    print("eu fmt int grp:", locale.format_string("%d", 1234567, grouping=True))
    print("eu fmt float grp:", locale.format_string("%.2f", 1234567.89, grouping=True))
    print("eu fmt width:", repr(locale.format_string("%12.2f", 1234.5, grouping=True)))
    print("eu currency pos:", locale.currency(1234567.89, grouping=True))
    print("eu currency neg:", locale.currency(-1234567.89, grouping=True))
    print("eu currency intl:", locale.currency(-1234.56, grouping=True, international=True))
    print("eu currency nosym:", locale.currency(1234.56, symbol=False, grouping=True))
    print("eu localize:", locale.localize("1234567.89", grouping=True))
    print("eu localize sci:", locale.localize("12345.67e+02", grouping=True))
    print("eu delocalize:", locale.delocalize("1.234.567,89"))
    print("eu atof:", locale.atof("1.234.567,89"))
    print("eu atoi:", locale.atoi("1.234.567"))
    print("eu str:", locale.str(1234.5))

    # 7. US-style currency with sign_posn variations (0=parens, 2=after, 3/4=adjacent to symbol)
    for sign_pos in (0, 1, 2, 3, 4):
        locale._override_localeconv = {
            "decimal_point": ".",
            "thousands_sep": ",",
            "grouping": [3, 3, 0],
            "mon_decimal_point": ".",
            "mon_thousands_sep": ",",
            "mon_grouping": [3, 3, 0],
            "currency_symbol": "$",
            "int_curr_symbol": "USD ",
            "int_frac_digits": 2,
            "frac_digits": 2,
            "p_cs_precedes": 1,
            "p_sep_by_space": 0,
            "n_cs_precedes": 1,
            "n_sep_by_space": 0,
            "p_sign_posn": sign_pos,
            "n_sign_posn": sign_pos,
            "positive_sign": "+",
            "negative_sign": "-",
        }
        print(
            "us sign_pos",
            sign_pos,
            locale.currency(1234.5, grouping=True),
            locale.currency(-1234.5, grouping=True),
        )

    # 8. Collation: strcoll & strxfrm
    print(
        "strcoll:",
        locale.strcoll("apple", "banana") < 0,
        locale.strcoll("banana", "apple") > 0,
        locale.strcoll("same", "same") == 0,
    )
    print("strxfrm order:", locale.strxfrm("apple") < locale.strxfrm("banana"))
    for fn, args in ((locale.strcoll, (1, "a")), (locale.strxfrm, (1,))):
        try:
            fn(*args)
        except TypeError:
            print(fn.__name__, "bad arg -> TypeError")
finally:
    locale._override_localeconv = {}
    locale.setlocale(locale.LC_CTYPE, saved_ctype)
