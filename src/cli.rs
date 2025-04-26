use clap::{Parser, ValueEnum};

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
    /// adjust contrast, eg. -c=-25 (default: 0)
    #[arg(short, long)]
    pub contrast: Option<f32>,
    /// adjust brightness, eg. -b=-60 (default: 0)
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
    /// directory with: wallpaper_dir/output/workspace_name.{jpg|png|...}
    pub wallpaper_dir: String,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum PixelFormat {
    Auto,
    Baseline,
}
