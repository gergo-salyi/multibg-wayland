# multibg-wayland

Set a different wallpaper for the background of each Sway / Hyprland / niri workspace

## News

Project is being renamed to multibg-wayland to signify support for Wayland compositors other than Sway. The name multibg-sway remains as an alias or redirect.

## Usage

    $ multibg-wayland <WALLPAPER_DIR>

Wallpapers should be arranged in the following directory structure:

    wallpaper_dir/output/workspace_name.{jpg|png|...}

Such as:

    ~/my_wallpapers/HDMI-A-1/1.jpg

In more detail:

- **wallpaper_dir**: A directory, this will be the command line argument

- **output**: A directory with the same name as a Wayland output such as eDP-1, HDMI-A-1
  - For multiple outputs this can be a symlink to the directory of an other output.
  - Get the name of current outputs from the compositor with these Sway / Hyprland / niri commands:

        $ swaymsg -t get_outputs
        $ hyprctl monitors
        $ niri msg outputs

- **workspace_name**: The name of the workspace, by default use the compositors assigned workspace numbers as names: 1, 2, 3, ..., 10
  - Can be the name of a named workspace usually defined in the config file of the compositor. (Renaming workspaces while multibg-workspace is running might not be supported yet.)
  - Can define a **fallback wallpaper** with the special name: **_default**
  - Can be a symlink to the wallpaper of an other workspace

### Example

For one having a laptop with a built-in display eDP-1 and an external monitor HDMI-A-1, wallpapers can be arranged such as:

    ~/my_wallpapers
        ├─ eDP-1
        │    ├─ _default.jpg
        │    ├─ 1.jpg
        │    ├─ 2.png
        │    └─ browser.jpg
        └─ HDMI-A-1
             ├─ 1.jpg
             └─ 3.png

Then start multibg-wayland:

    $ multibg-wayland ~/my_wallpapers

### Options

In case of errors we log to stderr and try to continue. Redirect stderr to a log file if necessary.

By default, without the `--gpu` option only CPU memory is used to store wallpapers, shared with the Wayland compositor. (All of this might be reported as memory used by the compositor process instead of our process.)

With the `--gpu` option set GPU memory (again, shared with the compositor) is used. This requires Vulkan loader and driver with Vulkan 1.1 or newer, and might save a few milliseconds latency on wallpaper switches avoiding the use of CPU memory and PCIe bandwidth. (I recommend to try this out, I just can't test it with many GPUs.)

The running Wayland compositor is autodetected based on environment variables. If this fails then try to set the `--compositor {sway|hyprland|niri}` command line option.

It is recommended to resize the wallpapers to the resolution of the output and color adjust with dedicated tools like imagemagick or gimp.

This app can do _some_ imperfect image processing at the expense of startup time. Wallpaper images with different resolution than their output are resized (with high quality filter but incorrect gamma) to _fill_ the output. Contrast and brightness (on some bad arbitrary scale) might be adjusted such as:

    $ multibg-wayland --contrast=-25 --brightness=-60 ~/my_wallpapers

### Resource usage

For active outputs all wallpapers from the corresponding `wallpaper_dir/output` are loaded and stored uncompressed to enable fast wallpaper switching. Wallpapers with multiple symlinks pointing to it are only loaded once and shared. For example for 10 unique full HD wallpaper this means 10\*1920\*1080\*4 = 83 MB memory use.

## Installation

Requires `Rust`, get it from your package manager or from the official website: [https://www.rust-lang.org/tools/install](https://www.rust-lang.org/tools/install)

- Latest release (from [crates.io](https://crates.io/crates/multibg-wayland)) with Cargo install provided by Rust:

      $ cargo install --locked multibg-wayland

  Run `~/.cargo/bin/multibg-wayland`

- Directly from the current git source:

      $ git clone https://github.com/gergo-salyi/multibg-wayland.git
      $ cd multibg-wayland
      $ cargo build --release --locked

  Run `./target/release/multibg-wayland`

- For Arch Linux from AUR: [https://aur.archlinux.org/packages/multibg-wayland](https://aur.archlinux.org/packages/multibg-wayland)
  - eg. with paru

        $ paru -S multibg-wayland

## Bug reporting

Reports on any problems are appreciated, look for an existing or open a new issue at [https://github.com/gergo-salyi/multibg-wayland/issues](https://github.com/gergo-salyi/multibg-wayland/issues)

Please include a verbose log from you terminal by running with `RUST_BACKTRACE=1` and `RUST_LOG=info,multibg_wayland=trace` environment variables set, such as

    $ export RUST_BACKTRACE=1
    $ export RUST_LOG=info,multibg_wayland=trace
    $ multibg-wayland ~/my_wallpapers

If using the --gpu option also consider installing Vulkan validation layers from your distro. It which will be automatically enabled at the log levels defined above.

## Alternatives

- [swaybg](https://github.com/swaywm/swaybg)
- [swww](https://github.com/Horus645/swww)
- [wpaperd](https://github.com/danyspin97/wpaperd)
- [hyprpaper](https://github.com/hyprwm/hyprpaper)
- [mpvpaper](https://github.com/GhostNaN/mpvpaper)
- [oguri](https://github.com/vilhalmer/oguri)

## License

Source files in this project are distributed under MIT OR Apache-2.0

Objects resulting from building this project might be under GPL-3.0-or-later due to licenses of statically linked dependencies. Open an issue if you need compile time features gating such dependencies.
