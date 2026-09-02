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
