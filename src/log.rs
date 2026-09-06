use super::*;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum LogLevel { Off, Error, Warn, Info, Debug, Trace }

static mut LOG_LEVEL: LogLevel = LogLevel::Off;

pub unsafe fn set_log_level(log_level: LogLevel) {
    unsafe { LOG_LEVEL = log_level }
}

fn log_level() -> LogLevel {
    unsafe { LOG_LEVEL }
}

pub fn log_enabled(level: LogLevel) -> bool {
    log_level() >= level
}

pub struct StderrBuf {
    buf: [MaybeUninit<u8>; Self::CAP],
    len: usize,
    stderr_lock: StderrLock<'static>,
}

impl StderrBuf {
    const CAP: usize = 2048;

    fn new() -> Self {
        Self {
            buf: [const { MaybeUninit::uninit() }; Self::CAP],
            len: 0,
            stderr_lock: stderr().lock(),
        }
    }

    fn write(&mut self, buf: &[u8]) {
        let buf_len = buf.len();
        if buf_len > Self::CAP - self.len {
            cold_path();
            self.flush();
            if buf_len > Self::CAP {
                write_all(&mut self.stderr_lock, buf);
                return
            }
        }
        unsafe {
            ptr::copy_nonoverlapping(
                buf.as_ptr(),
                self.buf.get_unchecked_mut(self.len..).as_mut_ptr().cast(),
                buf_len,
            );
            self.len += buf_len;
        }
    }

    fn flush(&mut self) {
        write_all(
            &mut self.stderr_lock,
            unsafe { self.buf.get_unchecked(..self.len).assume_init_ref() },
        );
        self.len = 0;
    }

    fn push_str(&mut self, s: &str) {
        self.write(s.as_bytes());
    }
}

impl fmt::Write for StderrBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

fn write_all(stderr_lock: &mut StderrLock<'static>, buf: &[u8]) {
    stderr_lock.write_all(buf).unwrap()
}

fn eprint_with(f: impl FnOnce(&mut StderrBuf)) {
    let mut buf = StderrBuf::new();
    f(&mut buf);
    buf.flush();
}

fn eprintln_with(f: impl FnOnce(&mut StderrBuf)) {
    eprint_with(|buf| {
        f(buf);
        buf.push_str("\n");
    })
}

pub fn eprintln_fmt(args: fmt::Arguments<'_>) {
    eprintln_with(|buf| { buf.write_fmt(args).unwrap() })
}

#[inline(never)]
pub fn log(args: fmt::Arguments<'_>) {
    eprintln_fmt(args)
}

#[inline(never)]
pub fn log_error(args: fmt::Arguments<'_>) {
    eprintln_with(|buf| {
        buf.push_str("ERROR ");
        buf.write_fmt(args).unwrap();
    })
}

#[inline(never)]
pub fn log_warn(args: fmt::Arguments<'_>) {
    eprintln_with(|buf| {
        buf.push_str("WARNING ");
        buf.write_fmt(args).unwrap();
    })
}

macro_rules! error {
    ($($arg:tt)*) => {{
        if log_enabled(LogLevel::Error) {
            log_error(format_args!($($arg)*))
        }
    }}
}

macro_rules! warning {
    ($($arg:tt)*) => {{
        if log_enabled(LogLevel::Warn) {
           log_warn(format_args!($($arg)*))
        }
    }}
}

macro_rules! info {
    ($($arg:tt)*) => {{
        if log_enabled(LogLevel::Info) {
            log(format_args!($($arg)*))
        }
    }}
}

macro_rules! debug {
    ($($arg:tt)*) => {{
        if log_enabled(LogLevel::Debug) {
            log(format_args!($($arg)*))
        }
    }}
}

macro_rules! trace {
    ($($arg:tt)*) => {{
        if log_enabled(LogLevel::Trace) {
            log(format_args!($($arg)*))
        }
    }}
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
