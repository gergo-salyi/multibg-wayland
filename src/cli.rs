use super::*;

const HELP_TXT: &str = include_str!("help.txt");

const SHORTS: &[(u8, Handler)] = &[
    (b'V', Flag(Cli::flag_version)),
    (b'b', Opt(Cli::opt_brightness)),
    (b'c', Opt(Cli::opt_contrast)),
    (b'h', Flag(Cli::flag_help)),
];

const LONGS: &[(&str, Handler)] = &[
    ("gpu", Flag(Cli::flag_gpu)),
    ("help", Flag(Cli::flag_help)),
    ("no-gpu", Flag(Cli::flag_no_gpu)),
    ("version", Flag(Cli::flag_version)),
    ("contrast", Opt(Cli::opt_contrast)),
    ("log-level", Opt(Cli::opt_log_level)),
    ("brightness", Opt(Cli::opt_brightness)),
    ("compositor", Opt(Cli::opt_compositor)),
    ("pixelformat", Opt(Cli::opt_pixel_format)),
    ("list-outputs", Flag(Cli::flag_list_outputs)),
    ("level-input-max", Opt(Cli::opt_level_input_max)),
    ("level-input-min", Opt(Cli::opt_level_input_min)),
    ("level-output-max", Opt(Cli::opt_level_output_max)),
    ("level-output-min", Opt(Cli::opt_level_output_min)),
];

struct CliFlag(u32);

impl CliFlag {
    const HELP: Self = Self(1 << 0);
    const VERSION: Self = Self(1 << 1);
    const LIST_OUTPUTS: Self = Self(1 << 2);
    const GPU: Self = Self(1 << 3);
    const HAS_PIXEL_FORMAT: Self = Self(1 << 4);
    const HAS_BRIGHTNESS_CONTRAST: Self = Self(1 << 5);
    const HAS_LEVELS: Self = Self(1 << 6);
}

pub struct Cli {
    pub wallpaper_dir: Option<Box<Path>>,
    pub contrast: f32,
    pub brightness: i32,
    pub levels: Levels,
    pub compositor: Option<Compositor>,
    pub log_level: LogLevel,
    flags: CliFlag,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            wallpaper_dir: None,
            contrast: 0.0,
            brightness: 0,
            levels: Levels {
                output_min: 0.0,
                output_max: 1.0,
                input_min: 0.0,
                input_max: 1.0,
            },
            compositor: None,
            log_level: cfg_select! {
                debug_assertions => LogLevel::Debug,
                _ => LogLevel::Warn,
            },
            flags: CliFlag(0),
        }
    }
}

impl Cli {
    pub fn parse() -> Result<Self, ()> {
        let mut cli = Self::default();
        cli.parse_env()?;
        cli.parse_args()?;
        if cli.get(CliFlag::HELP) {
            stdout().lock().write_all(HELP_TXT.as_bytes()).unwrap();
        } else if cli.get(CliFlag::VERSION) {
            println!("{PKG_NAME} {PKG_VERSION}");
        }
        Ok(cli)
    }

    pub fn done(&self) -> bool {
        self.get(CliFlag::HELP) || self.get(CliFlag::VERSION)
    }

    pub fn list_outputs(&self) -> bool {
        self.get(CliFlag::LIST_OUTPUTS)
    }

    pub fn gpu(&self) -> bool {
        self.get(CliFlag::GPU)
    }

    pub fn has_pixel_format(&self) -> bool {
        self.get(CliFlag::HAS_PIXEL_FORMAT)
    }

    pub fn has_levels(&self) -> bool {
        self.get(CliFlag::HAS_LEVELS)
    }

    pub fn has_brightness_contrast(&self) -> bool {
        self.get(CliFlag::HAS_BRIGHTNESS_CONTRAST)
    }

    fn get(&self, flag: CliFlag) -> bool {
        self.flags.0 & flag.0 == flag.0
    }

    fn set(&mut self, flag: CliFlag) {
        self.flags.0 |= flag.0;
    }

    fn unset(&mut self, flag: CliFlag) {
        self.flags.0 &= !flag.0;
    }

    fn positional(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let path = Path::new(OsStr::from_bytes(bytes));
        if let Some(wallpaper_dir) = self.wallpaper_dir.as_ref() {
            error!("duplicate wallpaper_dir argument {} after {}",
                quote_path(path), quote_path(wallpaper_dir));
            Err(())
        } else {
            self.wallpaper_dir = Some(Box::from(path));
            Ok(())
        }
    }

    fn flag_gpu(&mut self) -> Result<(), ()> {
        self.set(CliFlag::GPU);
        Ok(())
    }

    fn flag_no_gpu(&mut self) -> Result<(), ()> {
        self.unset(CliFlag::GPU);
        Ok(())
    }

    fn flag_help(&mut self) -> Result<(), ()> {
        self.set(CliFlag::HELP);
        Ok(())
    }

    fn flag_list_outputs(&mut self) -> Result<(), ()> {
        self.set(CliFlag::LIST_OUTPUTS);
        Ok(())
    }

    fn flag_version(&mut self) -> Result<(), ()> {
        self.set(CliFlag::VERSION);
        Ok(())
    }

    fn opt_brightness(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.get(CliFlag::HAS_LEVELS) {
            conflict_levels_legacy();
            return Err(())
        }
        if let Some(val) = parse_i32_in_range(bytes, -255..=255) {
            self.brightness = val;
            self.set(CliFlag::HAS_BRIGHTNESS_CONTRAST);
            Ok(())
        } else {
            invalid("brightness", bytes, fmt_range("-255", "255"));
            Err(())
        }
    }

    fn opt_compositor(&mut self, bytes: &[u8]) -> Result<(), ()> {
        self.compositor = match bytes {
            b"auto" => None,
            b"sway" => Some(Compositor::Sway),
            b"hyprland" => Some(Compositor::Hyprland),
            b"niri" => Some(Compositor::Niri),
            other => {
                invalid("compositor", other,
                    fmt_list(&["auto", "sway", "hyprland", "niri"]));
                return Err(())
            },
        };
        Ok(())
    }

    fn opt_contrast(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.get(CliFlag::HAS_LEVELS) {
            conflict_levels_legacy();
            return Err(())
        }
        if let Some(val) = parse_f32_in_range(bytes, -100.0..=f32::MAX) {
            self.contrast = val;
            self.set(CliFlag::HAS_BRIGHTNESS_CONTRAST);
            Ok(())
        } else {
            invalid("contrast", bytes, fmt_range("-100.0", "f32::MAX"));
            Err(())
        }
    }

    fn opt_level_input_max(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.get(CliFlag::HAS_BRIGHTNESS_CONTRAST) {
            conflict_levels_legacy();
            return Err(())
        }
        if let Some(val) = parse_f32_in_range(
            bytes,
            self.levels.input_min..=1.0,
        ) {
            self.levels.input_max = val;
            self.set(CliFlag::HAS_LEVELS);
            Ok(())
        } else {
            invalid("level-input-max", bytes,
                fmt_range("level-input-min", "1.0"));
            Err(())
        }
    }

    fn opt_level_input_min(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.get(CliFlag::HAS_BRIGHTNESS_CONTRAST) {
            conflict_levels_legacy();
            return Err(())
        }
        if let Some(val) = parse_f32_in_range(
            bytes,
            0.0..=self.levels.input_max,
        ) {
            self.levels.input_min = val;
            self.set(CliFlag::HAS_LEVELS);
            Ok(())
        } else {
            invalid("level-input-min", bytes,
                fmt_range("0.0", "level-input-max"));
            Err(())
        }
    }

    fn opt_level_output_max(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.get(CliFlag::HAS_BRIGHTNESS_CONTRAST) {
            conflict_levels_legacy();
            return Err(())
        }
        if let Some(val) = parse_f32_in_range(bytes, 0.0..=1.0) {
            self.levels.output_max = val;
            self.set(CliFlag::HAS_LEVELS);
            Ok(())
        } else {
            invalid("level-output-max", bytes, fmt_range("0.0", "1.0"));
            Err(())
        }
    }

    fn opt_level_output_min(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.get(CliFlag::HAS_BRIGHTNESS_CONTRAST) {
            conflict_levels_legacy();
            return Err(())
        }
        if let Some(val) = parse_f32_in_range(bytes, 0.0..=1.0) {
            self.levels.output_min = val;
            self.set(CliFlag::HAS_LEVELS);
            Ok(())
        } else {
            invalid("level-output-min", bytes, fmt_range("0.0", "1.0"));
            Err(())
        }
    }

    fn opt_log_level(&mut self, bytes: &[u8]) -> Result<(), ()> {
        match bytes {
            b"off" => self.log_level = LogLevel::Off,
            b"error" => self.log_level = LogLevel::Error,
            b"warn" => self.log_level = LogLevel::Warn,
            b"info" => self.log_level = LogLevel::Info,
            b"debug" => self.log_level = LogLevel::Debug,
            b"trace" => self.log_level = LogLevel::Trace,
            other => {
                invalid("log-level", other, fmt_list(&[
                    "off", "error", "warn", "info", "debug", "trace"]));
                return Err(())
            },
        };
        Ok(())
    }

    fn opt_pixel_format(&mut self, bytes: &[u8]) -> Result<(), ()> {
        match bytes {
            b"auto" | b"baseline" => self.set(CliFlag::HAS_PIXEL_FORMAT),
            other => {
                invalid("pixelformat", other, fmt_list(&["auto", "baseline"]));
                return Err(())
            },
        };
        Ok(())
    }

    fn parse_env(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn parse_args(&mut self) -> Result<(), ()> {
        let mut args = args_os()
            .map(|os_string| os_string.into_vec())
            .skip(1);
        let mut seen_dashdash = false;
        while let Some(arg) = args.next() {
            let mut arg = arg.as_slice();
            if seen_dashdash || !arg.starts_with(b"-") || arg == b"-" {
                self.positional(arg)?;
                continue
            }
            arg = &arg[1..];
            if !arg.starts_with(b"-") {
                self.parse_shorts(arg, &mut args)?;
                continue
            }
            arg = &arg[1..];
            if arg.is_empty() {
                seen_dashdash = true;
                continue
            }
            self.parse_long(arg, &mut args)?;
        }
        Ok(())
    }

    fn parse_shorts(
        &mut self,
        mut arg: &[u8],
        args: &mut impl Iterator<Item = Vec<u8>>,
    ) -> Result<(), ()> {
        while let Some(&short) = arg.first() {
            arg = &arg[1..];
            let Some(handler) = short_handler(short) else {
                unrecognized("-", &[short]);
                return Err(())
            };
            match handler {
                Flag(flag_fn) => {
                    if let Some(b'=') = arg.first() {
                        excess_value("-", &[short], &arg[1..]);
                        return Err(())
                    }
                    flag_fn(self)?;
                },
                Opt(opt_fn) => {
                    if let Some(b'=') = arg.first() {
                        opt_fn(self, &arg[1..])?;
                    } else if arg.is_empty() {
                        let Some(val) = args.next() else {
                            missing_value("-", &[short]);
                            return Err(())
                        };
                        opt_fn(self, &val)?;
                    } else {
                        opt_fn(self, arg)?;
                    }
                    return Ok(())
                }
            }
        }
        Ok(())
    }

    fn parse_long(
        &mut self,
        mut arg: &[u8],
        args: &mut impl Iterator<Item = Vec<u8>>,
    ) -> Result<(), ()> {
        let mut value = None;
        if let Some(idx) = memchr(arg, b'=') {
            value = Some(&arg[idx + 1..]);
            arg = &arg[..idx];
        }
        let Some(handler) = long_handler(arg) else {
            unrecognized("--", arg);
            return Err(())
        };
        match handler {
            Flag(flag_fn) => {
                if let Some(val) = value {
                    excess_value("--", arg, val);
                    return Err(())
                }
                flag_fn(self)?;
            },
            Opt(opt_fn) => {
                if let Some(val) = value {
                    opt_fn(self, val)?;
                } else {
                    let Some(val) = args.next() else {
                        missing_value("--", arg);
                        return Err(())
                    };
                    opt_fn(self, &val)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Handler {
    Flag(fn(&mut Cli) -> Result<(), ()>),
    Opt(fn(&mut Cli, &[u8]) -> Result<(), ()>),
}
use Handler::{Flag, Opt};

fn short_handler(short: u8) -> Option<Handler> {
    let idx = SHORTS.binary_search_by_key(&short, |elem| elem.0).ok()?;
    Some(SHORTS[idx].1)
}

fn long_handler(long: &[u8]) -> Option<Handler> {
    let idx = LONGS.binary_search_by(|elem|
        elem.0.len().cmp(&long.len())
            .then_with(|| elem.0.as_bytes().cmp(long))
    ).ok()?;
    Some(LONGS[idx].1)
}

fn parse_i32_in_range(
    bytes: &[u8],
    range: impl RangeBounds<i32>,
) -> Option<i32> {
    let num = str::from_utf8(bytes).ok()?.parse::<i32>().ok()?;
    if range.contains(&num) {
        Some(num)
    } else {
        None
    }
}

fn parse_f32_in_range(
    bytes: &[u8],
    range: impl RangeBounds<f32>,
) -> Option<f32> {
    let num = str::from_utf8(bytes).ok()?.parse::<f32>().ok()?;
    if range.contains(&num) {
        Some(num)
    } else {
        None
    }
}

#[cold]
fn print_error(message: impl Display) {
    eprintln!(
        "{PKG_NAME} {PKG_VERSION}\n\
        ERROR {message}\n\
        Try '{PKG_NAME} --help' for more information.",
    );
}

macro_rules! error {
    ($($arg:tt)*) => { print_error(format_args!($($arg)*)) }
}
use error;

fn unrecognized(dash: &str, opt: &[u8]) {
    error!("unrecognized option {}", quote_prefixed_bytes(dash, opt));
}

fn excess_value(dash: &str, flag: &[u8], val: &[u8]) {
    error!("flag {}{} takes no value but found {}",
        dash, str::from_utf8(flag).unwrap(), quote_bytes(val));
}

fn missing_value(dash: &str, opt: &[u8]) {
    error!("missing value for option {}{}",
        dash, str::from_utf8(opt).unwrap());
}

fn invalid(opt: &str, bytes: &[u8], valid: impl Display) {
    error!("invalid {} value {}, expected: {}",
        opt, quote_bytes(bytes), valid);
}

fn conflict_levels_legacy() {
    error!("options --level-* are mutually exclusive with \
        legacy options brightness and contrast");
}

fn fmt_list(values: &[&str]) -> impl Display {
    fmt::from_fn(|f| {
        let mut vals = values.iter();
        if let Some(val) = vals.next() {
            f.write_str(val)?;
        }
        for val in vals {
            f.write_str(", ")?;
            f.write_str(val)?;
        }
        Ok(())
    })
}

fn fmt_range(start: &str, end: &str) -> impl Display {
    fmt::from_fn(move |f| write!(f, "in range [{start}, {end}]"))
}

const _: () = {
    let mut i = 1;
    while i < SHORTS.len() {
        if SHORTS[i - 1].0 > SHORTS[i].0 {
            panic!("SHORTS not sorted")
        }
        i += 1;
    }

    let mut i = 1;
    while i < LONGS.len() {
        let left = LONGS[i - 1].0.as_bytes();
        let right = LONGS[i].0.as_bytes();
        if left.len() > right.len() {
            panic!("LONGS not sorted")
        } else if left.len() == right.len() {
            let mut j = 0;
            while j < left.len() {
                if left[j] < right[j] {
                    break
                } else if left[j] > right[j] {
                    panic!("LONGS not sorted")
                }
                j += 1;
            }
        }
        i += 1;
    }
};
