# multibg-sway

Set a different wallpaper for the background of each Sway workspace

## Usage

    $ multibg-sway <WALLPAPER_DIR>

Wallpapers should be arranged in the following directory structure:

    wallpaper_dir/output/workspace_name.{jpg|png|...}

Eg.

    ~/my_wallpapers/HDMI-A-1/1.jpg

In more detail:

- **wallpaper_dir**: A directory, this will be the argument for the multibg-sway command

- **output**: A directory with the same name as a sway output eg. eDP-1, HDMI-A-1
  - If one has multiple outputs with the same resolution this can be a symlink to the directory of the other output. 
  - To get the name of current outputs from sway one may run:

        $ swaymsg -t get_outputs

- **workspace_name**: The name of the sway workspace, by sway defaults: 1, 2, 3, ..., 10
  - Can be a manually defined workspace name (eg. in sway config), but renaming workspaces while multibg-sway is running is not supported currently
  - Can define a **fallback wallpaper** with the special name: **_default**
  - Can be a symlink to use a wallpaper image for multiple workspaces

Wallpaper images are now automatically resized at startup to _fill_ the output. Still it is better to have wallpaper images the same resolution as the output, which automatically avoids resizing operations and decreases startup time.

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

Then start multibg_sway:

    $ multibg-sway ~/my_wallpapers

It is recommended to edit the wallpaper images in a dedicated image editor. Nevertheless the contrast and brightness might be adjusted here:

    $ multibg-sway --contrast=-25 --brightness=-60 ~/my_wallpapers

In case of errors multibg-sway logs to stderr and tries to continue. One may wish to redirect stderr if multibg-sway is being run as a daemon.

### Resource usage

Loaded wallpapers are stored uncompressed to enable fast wallpaper switching with nearly zero CPU use. For example for 10 full HD wallpaper this means 10\*1920\*1080\*4 = 83 MB graphics memory use.

Because multibg-sway doesn't have its own GPU context and manages graphics memory through sway, all this usage might be reported as additional memory used by the sway process.

## Installation

Requires `Rust`, get it from your package manager or from the official website: [https://www.rust-lang.org/tools/install](https://www.rust-lang.org/tools/install)

- Latest release (from [crates.io](https://crates.io/crates/multibg-sway)) with Cargo install provided by Rust:

      $ cargo install --locked multibg-sway

  Run `~/.cargo/bin/multibg-sway`

- Directly from the current git source:

      $ git clone https://github.com/gergo-salyi/multibg-sway.git
      $ cd multibg-sway
      $ cargo build --release --locked

  Run `./target/release/multibg-sway`

- For Arch Linux from AUR: [https://aur.archlinux.org/packages/multibg-sway](https://aur.archlinux.org/packages/multibg-sway)
  - eg. with paru

        $ paru -S multibg-sway

## Bug reporting

Reports on any problems are appreciated, look for an existing or open a new issue at [https://github.com/gergo-salyi/multibg-sway/issues](https://github.com/gergo-salyi/multibg-sway/issues)

Please include a verbose log from you terminal by running with `RUST_BACKTRACE=1` and `RUST_LOG=trace` environment variables set, such as

    $ RUST_BACKTRACE=1 RUST_LOG=trace multibg-sway ~/my_wallpapers

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
