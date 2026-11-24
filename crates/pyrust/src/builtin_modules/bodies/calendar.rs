// `calendar` module — body for the `calendar @inject` entry in
// `pyrust_builtin_modules!` (issue #3185).
//
// CPython 3.12's `calendar` is a pure-Python module providing general
// calendar-related functions, `Day` / `Month` IntEnums, `Calendar`,
// `TextCalendar`, and `HTMLCalendar`.
//
// Reference: <https://docs.python.org/3/library/calendar.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `calendar` module.
const CALENDAR_PY_SOURCE: &str = include_str!("calendar_py.py");

/// Public and compatibility names exported from `CALENDAR_PY_SOURCE` onto the
/// `calendar` module.
const CALENDAR_PY_EXPORTS: &[&str] = &[
    "__all__",
    "error",
    "IllegalMonthError",
    "IllegalWeekdayError",
    "__getattr__",
    "Month",
    "JANUARY",
    "FEBRUARY",
    "MARCH",
    "APRIL",
    "MAY",
    "JUNE",
    "JULY",
    "AUGUST",
    "SEPTEMBER",
    "OCTOBER",
    "NOVEMBER",
    "DECEMBER",
    "Day",
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
    "mdays",
    "day_name",
    "day_abbr",
    "month_name",
    "month_abbr",
    "isleap",
    "leapdays",
    "weekday",
    "monthrange",
    "Calendar",
    "TextCalendar",
    "HTMLCalendar",
    "LocaleTextCalendar",
    "LocaleHTMLCalendar",
    "c",
    "firstweekday",
    "setfirstweekday",
    "monthcalendar",
    "prweek",
    "week",
    "weekheader",
    "prmonth",
    "month",
    "calendar",
    "prcal",
    "format",
    "formatstring",
    "EPOCH",
    "timegm",
];

/// Execute `CALENDAR_PY_SOURCE` once and copy its exported attributes onto the
/// `calendar` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(CALENDAR_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("calendar: exec namespace not a dict".into()))?;
    for name in CALENDAR_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("calendar"));
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
