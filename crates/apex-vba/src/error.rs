//! Errors of VBA code: syntax errors found when the modules are read and
//! run-time errors with the numbers VBA gives them (what `Err.Number`
//! shows and `On Error` catches), with the module and line they come from.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    /// VBA error number (0 for syntax errors).
    pub number: i32,
    pub message: String,
    /// Module and line (1-based, as the editor numbers them) where it
    /// happened; empty and 0 until known.
    pub module: String,
    pub line: u32,
    /// Stops the macro even under `On Error Resume Next` (limits, End).
    pub fatal: bool,
}

pub type R<T> = Result<T, Error>;

impl Error {
    pub fn new(number: i32, message: impl Into<String>) -> Error {
        Error { number, message: message.into(), module: String::new(), line: 0, fatal: false }
    }

    pub fn syntax(module: &str, line: u32, message: impl Into<String>) -> Error {
        Error { number: 0, message: message.into(), module: module.to_string(), line, fatal: true }
    }

    /// Sets where it happened unless already known.
    pub fn at(mut self, module: &str, line: u32) -> Error {
        if self.line == 0 {
            self.module = module.to_string();
            self.line = line;
        }
        self
    }

    pub fn fatal(mut self) -> Error {
        self.fatal = true;
        self
    }

    pub fn invalid_call() -> Error {
        Error::new(5, "Invalid procedure call or argument")
    }

    pub fn overflow() -> Error {
        Error::new(6, "Overflow")
    }

    pub fn subscript() -> Error {
        Error::new(9, "Subscript out of range")
    }

    pub fn div_zero() -> Error {
        Error::new(11, "Division by zero")
    }

    pub fn type_mismatch() -> Error {
        Error::new(13, "Type mismatch")
    }

    pub fn not_set() -> Error {
        Error::new(91, "Object variable or With block variable not set")
    }

    pub fn object_required() -> Error {
        Error::new(424, "Object required")
    }

    pub fn no_member(name: &str) -> Error {
        Error::new(438, format!("Object doesn't support this property or method: {name}"))
    }

    pub fn app(message: impl Into<String>) -> Error {
        Error::new(1004, message)
    }

    /// Something ApexDesk leaves out on purpose (UserForms, Declare,
    /// automating other applications).
    pub fn unsupported(what: &str) -> Error {
        Error::new(445, format!("Not supported in ApexDesk: {what}"))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.number != 0 {
            write!(f, "{} ({})", self.message, self.number)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for Error {}
