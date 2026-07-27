use num_bigint::BigInt;

use crate::error::{PyError, Result};
use crate::token::{FStringPart, Token};

// Lexer implementation grouped by lexical responsibility.

include!("lexer/scanner.rs");
include!("lexer/indentation.rs");
include!("lexer/numbers.rs");
include!("lexer/identifiers.rs");
include!("lexer/bytes.rs");
include!("lexer/strings.rs");
include!("lexer/fstrings.rs");
include!("lexer/escapes.rs");
include!("lexer/tests.rs");
