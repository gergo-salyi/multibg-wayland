use clap::{Parser, ValueEnum};

#[derive(Parser)]
#[command(author, version, long_about = None, about = "\
Set a different wallpaper for the background of each Sway workspace

    $ multibg-sway <WALLPAPER_DIR> [--compositor <COMPOSITOR>]

Wallpapers should be arranged in the following directory structure:

    wallpaper_dir/output/workspace_name.{jpg|png|...}

Eg.

    ~/my_wallpapers/HDMI-A-1/1.jpg

In more detail:

- wallpaper_dir: A directory, this will be the argument
    for the multibg-sway command

- output: A directory with the same name as a sway output
    eg. eDP-1, HDMI-A-1

    - If one has multiple outputs with the same resolution this
      can be a symlink to the directory of the other output

    - To get the name of current outputs from sway one may run:

        $ swaymsg -t get_outputs

- workspace_name: The name of the sway workspace,
    by sway defaults: 1, 2, 3, ..., 10

    - Can be a manually defined workspace name (eg. in sway config),
      but renaming workspaces while multibg-sway is running
      is not supported currently

    - Can define a fallback wallpaper with the special name: _default

    - Can be a symlink to use a wallpaper image for multiple workspaces

Wallpaper images are now automatically resized at startup to fill the output.
Still it is better to have wallpaper images the same resolution as the output,
which automatically avoids resizing operations and decreases startup time.

Example:

For one having a laptop with a built-in display eDP-1
and an external monitor HDMI-A-1, wallpapers can be arranged such as:

    ~/my_wallpapers
        ├─ eDP-1
        │    ├─ _default.jpg
        │    ├─ 1.jpg
        │    ├─ 2.png
        │    └─ browser.jpg
        └─ HDMI-A-1
             ├─ 1.jpg
             └─ 3.png

Then start multibg_sway:

    $ multibg-sway ~/my_wallpapers

It is recommended to edit the wallpaper images in a dedicated image editor.
Nevertheless the contrast and brightness might be adjusted here:

    $ multibg-sway --contrast=-25 --brightness=-60 ~/my_wallpapers

In case of errors multibg-sway logs to stderr and tries to continue.
One may wish to redirect stderr if multibg-sway is being run as a daemon.


multibg-sway supports multiple compositors,
currently only sway, hyprland and niri.
It tries to autodetect the compositor based on environment variables,
defaulting to sway if that fails.
Pass --compositor {hyprland|niri} to ensure it can to talk to them")]
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
