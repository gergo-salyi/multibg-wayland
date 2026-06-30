use std::fmt::Write as _;

use anyhow::bail;
use clap::{Parser, ValueEnum};
use log::warn;

use crate::Levels;

#[derive(Parser)]
#[command(author, version, long_about = None, about = "\
Set a different wallpaper for the background of each Sway / Hyprland / niri workspace

    $ multibg-wayland <WALLPAPER_DIR>

Wallpapers should be arranged in the following directory structure:

    wallpaper_dir/output/workspace_name.{jpg|png|...}

Such as:

    ~/my_wallpapers
        ├─ eDP-1
        │    ├─ _default.jpg
        │    ├─ 1.jpg
        │    ├─ 2.png
        │    └─ browser.jpg
        └─ HDMI-A-1
             ├─ 1.jpg
             └─ 3.png

For more details please read the README at:
https://github.com/gergo-salyi/multibg-wayland/blob/master/README.md")]
pub struct Cli {
    /// Brighten images by mapping colors to a range brighter than limit
    /// (useful for light themes).
    /// Takes a fraction in range [0.0, 1.0]. (default: 0.0)
    #[arg(long)]
    pub level_output_min: Option<f32>,
    /// Darken images by mapping colors to a range darker than limit
    /// (useful for dark themes).
    /// Takes a fraction in range [0.0, 1.0]. (default: 1.0)
    #[arg(long)]
    pub level_output_max: Option<f32>,
    /// Stretch contrast by clipping colors darker than limit to black.
    /// Takes a fraction in range [0.0, level-input-max]. (default: 0.0)
    #[arg(long)]
    pub level_input_min: Option<f32>,
    /// Stretch contrast by clipping colors brighter than limit to saturation.
    /// Takes a fraction in range [level-input-min, 1.0]. (default: 1.0)
    #[arg(long)]
    pub level_input_max: Option<f32>,
    /// DEPRECATED: use the --level-* options instead
    /// (brightness-contrast is difficult to use correctly).
    /// Adjusts contrast, eg. -c=-25 (default: 0)
    #[arg(short, long)]
    pub contrast: Option<f32>,
    /// DEPRECATED: use the --level-* options instead
    /// (brightness-contrast is difficult to use correctly).
    /// Adjusts brightness, eg. -b=-60 (default: 0)
    #[arg(short, long)]
    pub brightness: Option<i32>,
    /// wl_buffer pixel format (default: auto)
    #[arg(long)]
    pub pixelformat: Option<PixelFormat>,
    /// Wayland compositor to connect (autodetect by default)
    #[arg(long)]
    pub compositor: Option<crate::compositors::Compositor>,
    /// upload and serve wallpapers from GPU memory using Vulkan
    #[arg(long)]
    pub gpu: bool,
    /// list output names and make-model-serials and exit
    #[arg(long)]
    pub list_outputs: bool,
    /// directory with: wallpaper_dir/output/workspace_name.{jpg|png|...}
    #[arg(default_value = ".")]
    pub wallpaper_dir: String,
}

impl Cli {
    pub fn levels(&self) -> anyhow::Result<Option<Levels>> {
        let has_levels = self.level_output_min.is_some()
            || self.level_output_max.is_some()
            || self.level_input_min.is_some()
            || self.level_input_max.is_some();
        let has_brightness_contrast = self.brightness.is_some()
            || self.contrast.is_some();
        if has_levels && has_brightness_contrast {
            bail!("Options --level-* are mutually exclusive with \
                legacy options --brightness and --contrast");
        } else if has_levels {
            let input_min = self.level_input_min.unwrap_or(0.0);
            let input_max = self.level_input_max.unwrap_or(1.0);
            let output_min = self.level_output_min.unwrap_or(0.0);
            let output_max = self.level_output_max.unwrap_or(1.0);
            if input_min == 0.0 && input_max == 1.0
                && output_min == 0.0 && output_max == 1.0
            {
                return Ok(None)
            }
            if !(0.0..=input_max).contains(&input_min) {
                bail!("Option --level-input-min must be \
                    a fraction in range [0.0, level-input-max]");
            }
            if !(input_min..=1.0).contains(&input_max) {
                bail!("Option --level-input-max must be \
                    a fraction in range [level-input-min, 1.0]");
            }
            if !(0.0..=1.0).contains(&output_min) {
                bail!("Option --level-output-min must be \
                    a fraction in range [0.0, 1.0]");
            }
            if !(0.0..=1.0).contains(&output_max) {
                bail!("Option --level-output-max must be \
                    a fraction in range [0.0, 1.0]");
            }
            Ok(Some(Levels { input_min, input_max, output_min, output_max }))
        } else if has_brightness_contrast {
            let brightness = self.brightness.unwrap_or(0);
            let contrast = self.contrast.unwrap_or(0.0);
            if brightness == 0 && contrast == 0.0 {
                return Ok(None)
            }
            let levels = Levels::from_legacy(brightness, contrast);
            warn_brightness_contrast(brightness, contrast, &levels);
            Ok(Some(levels))
        } else {
            Ok(None)
        }
    }
}

fn warn_brightness_contrast(
    brightness: i32,
    contrast: f32,
    levels: &Levels,
) {
    let mut w = String::with_capacity(2000);
    write!(w, "Options --brightness and --contrast are deprecated, \
        they are difficult to use correctly. Use the --level-* \
        options instead, see --help. Current").unwrap();
    if brightness != 0 {
        write!(w, " --brightness={}", brightness).unwrap();
    }
    if contrast != 0.0 {
        write!(w, " --contrast={}", contrast).unwrap();
    }
    write!(w, " is equivalent to").unwrap();
    if levels.output_min != 0.0 {
        write!(w, " --level-output-min={}", levels.output_min).unwrap();
    }
    if levels.output_max != 1.0 {
        write!(w, " --level-output-max={}", levels.output_max).unwrap();
    }
    if levels.input_min != 0.0 {
        write!(w, " --level-input-min={}", levels.input_min).unwrap();
    }
    if levels.input_max != 1.0 {
        write!(w, " --level-input-max={}", levels.input_max).unwrap();
    }
    warn!("{}", w);
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum PixelFormat {
    Auto,
    Baseline,
}
