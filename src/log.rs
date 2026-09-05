use super::*;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum LogLevel { Off, Error, Warn, Info, Debug, Trace }

impl LogLevel {
    pub fn error(self) -> bool {
        self >= Self::Error
    }

    pub fn warn(self) -> bool {
        self >= Self::Warn
    }

    pub fn info(self) -> bool {
        self >= Self::Info
    }

    pub fn debug(self) -> bool {
        self >= Self::Debug
    }

    pub fn trace(self) -> bool {
        self >= Self::Trace
    }
}

static mut LOG_LEVEL: LogLevel = LogLevel::Off;

pub fn log_level() -> LogLevel {
    unsafe { LOG_LEVEL }
}

pub unsafe fn set_log_level(log_level: LogLevel) {
    unsafe { LOG_LEVEL = log_level }
}

macro_rules! error {
    ($($arg:tt)*) => {
        if $crate::log::log_level().error() {
            eprintln!("ERROR {}", format_args!($($arg)*));
        }
    }
}

macro_rules! warning {
    ($($arg:tt)*) => {
        if $crate::log::log_level().warn() {
            eprintln!("WARNING {}", format_args!($($arg)*));
        }
    }
}

macro_rules! info {
    ($($arg:tt)*) => {
        if $crate::log::log_level().info() {
            eprintln!($($arg)*);
        }
    }
}

macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::log::log_level().debug() {
            eprintln!($($arg)*);
        }
    }
}

macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::log::log_level().trace() {
            eprintln!($($arg)*);
        }
    }
}

pub(crate) use {error, warning as warn, info, debug, trace};

pub fn quote_str(s: &str) -> impl Display {
    fmt::from_fn(|f| fmt_quote_str(f, s))
}

pub fn quote_bytes(bytes: &[u8]) -> impl Display {
    fmt::from_fn(|f| fmt_quote_prefixed_bytes(f, "", bytes))
}

pub fn quote_prefixed_bytes(prefix: &str, bytes: &[u8]) -> impl Display {
    fmt::from_fn(|f| fmt_quote_prefixed_bytes(f, prefix, bytes))
}

pub fn quote_path(path: &Path) -> impl Display {
    quote_bytes(path.as_os_str().as_bytes())
}

fn fmt_quote_str(f: &mut Formatter, s: &str) -> fmt::Result {
    if !s.as_bytes().iter()
        .any(|b| matches!(b, b'\0'..=b'\x1F' | b'\'' | b'\x7F'))
    {
        return write!(f, "'{s}'")
    }
    write!(f, "$'")?;
    for chr in s.chars() {
        fmt_quoted_chr(f, chr)?;
    }
    f.write_char('\'')
}

fn fmt_quote_prefixed_bytes(
    f: &mut Formatter,
    prefix: &str,
    bytes: &[u8],
) -> fmt::Result {
    let mut first = true;
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid();
        let invalid = chunk.invalid();
        if first {
            if invalid.is_empty() && !bytes.iter()
                .any(|b| matches!(b, b'\0'..=b'\x1F' | b'\'' | b'\x7F'))
            {
                return write!(f, "'{prefix}{valid}'")
            }
            write!(f, "$'{prefix}")?;
            first = false;
        }
        for chr in valid.chars() {
            fmt_quoted_chr(f, chr)?;
        }
        for &byte in invalid {
            fmt_quoted_byte(f, byte)?;
        }
    }
    if first {
        write!(f, "'{prefix}")?;
    }
    f.write_char('\'')
}

fn fmt_quoted_chr(f: &mut Formatter, chr: char) -> fmt::Result {
    let escape = match chr {
        '\"' => Some('\"'),
        '\'' => Some('\''),
        '\\' => Some('\\'),
        '\n' => Some('n'),
        '\r' => Some('r'),
        '\t' => Some('t'),
        '\0'..='\x1f' | '\x7f' => None,
        other => {
            return f.write_char(other)
        }
    };
    if let Some(e) = escape {
        write!(f, "\\{e}")
    } else {
        fmt_quoted_byte(f, chr as u8)
    }
}

fn fmt_quoted_byte(f: &mut Formatter, byte: u8) -> fmt::Result {
    write!(f, "\\x{:02x}", byte)
}
