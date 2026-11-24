# CPython 3.12 parity tests for `calendar` (issue #3185).

import io
import sys
import contextlib
import datetime
import calendar
from calendar import (
    Calendar,
    TextCalendar,
    HTMLCalendar,
    Day,
    Month,
    MONDAY,
    TUESDAY,
    WEDNESDAY,
    THURSDAY,
    FRIDAY,
    SATURDAY,
    SUNDAY,
    JANUARY,
    FEBRUARY,
    MARCH,
    APRIL,
    MAY,
    JUNE,
    JULY,
    AUGUST,
    SEPTEMBER,
    OCTOBER,
    NOVEMBER,
    DECEMBER,
    day_name,
    day_abbr,
    month_name,
    month_abbr,
    IllegalMonthError,
    IllegalWeekdayError,
    isleap,
    leapdays,
    weekday,
    weekheader,
    monthrange,
    monthcalendar,
    month,
    prmonth,
    calendar as format_cal,
    prcal,
    setfirstweekday,
    firstweekday,
    timegm,
)

# 1. Module identity, __all__, enums, constants
print("mod_id:", calendar is sys.modules["calendar"])
print("all:", calendar.__all__)
print("days:", list(Day), [int(d) for d in Day], [repr(d) for d in Day], [str(d) for d in Day])
print("months:", list(Month), [int(m) for m in Month], [repr(m) for m in Month], [str(m) for m in Month])
print("day_consts:", MONDAY is Day.MONDAY, SUNDAY is Day.SUNDAY, MONDAY == 0, SUNDAY == 6)
print("month_consts:", JANUARY is Month.JANUARY, DECEMBER is Month.DECEMBER, JANUARY == 1, DECEMBER == 12)
print("day_names:", len(day_name), list(day_name), day_name[1:4])
print("day_abbrs:", len(day_abbr), list(day_abbr), day_abbr[5:])
print("month_names:", len(month_name), list(month_name), month_name[1:4])
print("month_abbrs:", len(month_abbr), list(month_abbr), month_abbr[10:])

# 2. Exceptions
e_m = IllegalMonthError(13)
e_w = IllegalWeekdayError(7)
print("err_month:", isinstance(e_m, ValueError), e_m.month, str(e_m))
print("err_weekday:", isinstance(e_w, ValueError), e_w.weekday, str(e_w))

# 3. isleap, leapdays, weekday, monthrange
for y in [1900, 1996, 1999, 2000, 2004, 2023, 2024, 2100, 2400]:
    print("isleap:", y, isleap(y))
print("leapdays:", leapdays(1900, 2000), leapdays(2000, 2025), leapdays(2020, 2020), leapdays(2000, 2001))
print("weekday:", weekday(2024, 2, 29), repr(weekday(2024, 2, 29)), weekday(2001, 1, 1), weekday(1970, 1, 1))
print("weekday_out_of_range:", weekday(10000, 1, 1), weekday(0, 1, 1))
print("monthrange:", monthrange(2024, 2), monthrange(2023, 2), monthrange(2024, 7))
try:
    monthrange(2024, 0)
except IllegalMonthError as e:
    print("monthrange_0:", str(e))
try:
    monthrange(2024, 13)
except IllegalMonthError as e:
    print("monthrange_13:", str(e))

# 4. firstweekday / setfirstweekday / weekheader / monthcalendar
print("firstweekday_init:", firstweekday())
print("weekheader_2_3_9:", repr(weekheader(2)), repr(weekheader(3)), repr(weekheader(9)))
setfirstweekday(SUNDAY)
print("firstweekday_sun:", firstweekday())
print("weekheader_sun:", repr(weekheader(2)), repr(weekheader(3)))
print("monthcal_sun:", monthcalendar(2024, 2))
try:
    setfirstweekday(7)
except IllegalWeekdayError as e:
    print("setfirstweekday_err:", str(e))
setfirstweekday(MONDAY)
print("monthcal_mon:", monthcalendar(2024, 2))

# 5. timegm
print("timegm_epoch:", timegm((1970, 1, 1, 0, 0, 0)))
print("timegm_2024:", timegm((2024, 2, 29, 12, 34, 56)))
print("timegm_pre_epoch:", timegm((1969, 12, 31, 23, 59, 59)))
print("timegm_extra_fields:", timegm((2024, 1, 1, 0, 0, 0, 0, 1, 0)))

# 6. Calendar class methods
cal = Calendar(firstweekday=WEDNESDAY)
print("cal_fw:", cal.firstweekday)
print("iterweekdays:", list(cal.iterweekdays()))
print("itermonthdays:", list(cal.itermonthdays(2024, 2)))
print("itermonthdays2:", list(cal.itermonthdays2(2024, 2)))
print("itermonthdays3:", list(cal.itermonthdays3(2024, 2)))
print("itermonthdays4:", list(cal.itermonthdays4(2024, 2)))
dates = list(cal.itermonthdates(2024, 2))
print("itermonthdates_sample:", dates[0], dates[-1], len(dates), isinstance(dates[0], datetime.date))
print("monthdatescalendar_len:", len(cal.monthdatescalendar(2024, 2)))
print("monthdays2calendar:", cal.monthdays2calendar(2024, 2))
print("monthdayscalendar:", cal.monthdayscalendar(2024, 2))
print("yeardayscalendar_shape:", len(cal.yeardayscalendar(2024, width=4)), [len(r) for r in cal.yeardayscalendar(2024, width=4)])
print("yeardays2calendar_jan:", cal.yeardays2calendar(2024, width=3)[0][0])
print("yeardatescalendar_jan1:", cal.yeardatescalendar(2024, width=3)[0][0][0][0])

# 7. TextCalendar & module-level month / prmonth / calendar / prcal
tc = TextCalendar(firstweekday=SUNDAY)
print("text_month:\n" + tc.formatmonth(2024, 2))
print("text_month_wl:\n" + tc.formatmonth(2024, 2, w=3, l=2))
print("mod_month:\n" + month(2024, 2))
buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    prmonth(2024, 2)
print("prmonth_eq:", buf.getvalue() == month(2024, 2))

yr_str = format_cal(2024, w=2, l=1, c=6, m=3)
print("year_lines:", len(yr_str.splitlines()))
print("year_head:\n" + "\n".join(yr_str.splitlines()[:12]))
buf2 = io.StringIO()
with contextlib.redirect_stdout(buf2):
    prcal(2024, w=2, l=1, c=6, m=3)
print("prcal_eq:", buf2.getvalue() == yr_str)

# 8. HTMLCalendar
hc = HTMLCalendar(firstweekday=MONDAY)
print("html_month:\n" + hc.formatmonth(2024, 2))
print("html_month_noyear:\n" + hc.formatmonth(2024, 2, withyear=False))
hy = hc.formatyear(2024, width=4)
print("html_year_len:", len(hy), hy.startswith('<table border="0"'), hy.endswith("</table>"))
hyp = hc.formatyearpage(2024, width=3, css="custom.css", encoding="utf-8")
print("html_yearpage:", isinstance(hyp, bytes), b"custom.css" in hyp, b"Calendar for 2024" in hyp, len(hyp))
