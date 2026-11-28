//! The regular expressions CPython's `textwrap` compiles at import, verbatim
//! (`.pattern` of the 3.12.3 objects), exposed as `TextWrapper.wordsep_re` & co.

pub(super) const WORDSEP_PATTERN: &str = "\n        ( # any whitespace\n          [\\\t\\\n\\\u{000b}\\\u{000c}\\\r\\ ]+\n        | # em-dash between words\n          (?<=[\\w!\"\\'&.,?]) -{2,} (?=\\w)\n        | # word, possibly hyphenated\n          [^\\\t\\\n\\\u{000b}\\\u{000c}\\\r\\ ]+? (?:\n            # hyphenated word\n              -(?: (?<=[^\\d\\W]{2}-) | (?<=[^\\d\\W]-[^\\d\\W]-))\n              (?= [^\\d\\W] -? [^\\d\\W])\n            | # end of word\n              (?=[\\\t\\\n\\\u{000b}\\\u{000c}\\\r\\ ]|\\Z)\n            | # em-dash\n              (?<=[\\w!\"\\'&.,?]) (?=-{2,}\\w)\n            )\n        )";

pub(super) const WORDSEP_SIMPLE_PATTERN: &str = "([\\\t\\\n\\\u{000b}\\\u{000c}\\\r\\ ]+)";

pub(super) const SENTENCE_END_PATTERN: &str = "[a-z][\\.\\!\\?][\\\"\\']?\\Z";

pub(super) const WHITESPACE_ONLY_PATTERN: &str = "^[ \t]+$";

pub(super) const LEADING_WHITESPACE_PATTERN: &str = "(^[ \t]*)(?:[^ \t\n])";
