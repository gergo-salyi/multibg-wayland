# Changelog

## 0.2.3 - 2025-08-28
- Add support for using output make-model-serial strings as per-output wallpaper directory names [#18](https://github.com/gergo-salyi/multibg-wayland/issues/18)
- Add support for selecting wallpapers of named workspaces by workspace number as a fallback instead of the workspace name [#19](https://github.com/gergo-salyi/multibg-wayland/pull/19)
- Update dependencies

## 0.2.2 - 2025-06-15
- Fix Wayland protocol error invalid stride on wl_shm with bgr888 format [#17](https://github.com/gergo-salyi/multibg-wayland/issues/17)
- Update dependencies, notably zune-jpeg fixing a minor discoloration issue on decoded JPEG images

## 0.2.1 - 2025-06-01
- Fix niri compatibility [#16](https://github.com/gergo-salyi/multibg-wayland/issues/16)
- Update dependencies

## 0.2.0 - 2025-04-26

### Breaking changes
- Project is being renamed to multibg-wayland to signify support for Wayland compositors other than Sway
- Terminate gracefully on signals INT, HUP or TERM with exit code 0. A second of such signals still kills. USR1 and USR2 are reserved for future use
- License of the source files is still MIT OR Apache-2.0 but built objects now might be under GPL-3.0-or-later.
- Arch Linux package now depends on dav1d

### Other changes
- Added support for the Hyprland and niri wayland compositors
- Inside the wallpaper directory wallpapers symlinked to the same image are now loaded only once and shared saving memory use
- Correct docs about the memory type we use. We only used CPU memory and will only use CPU memory unless the new --gpu option is given.
- Add ability to store wallpapers in GPU memory with the --gpu command line option. Requires Vulkan 1.1 or newer. This might save a few milliseconds latency on wallpaper switches avoiding the use of CPU memory and PCIe bandwidth
- Added support for AVIF images. Requires dav1d native dependency and the avif compile time feature (disabled by default for from source builds, enabled by default for Arch Linux package)
- Update README for the new features
- Update dependencies, require Rust compiler version 1.82 or newer
- Lots of internal changes supporting all the above

## 0.1.10 - 2024-11-17
- Fix sometimes disappearing mouse cursor above wallpapers
- Add small clarifications to README
- Update Arch Linux PKGBUILD to follow their Rust package guidelines
- Add minimum supported Rust version so Cargo can enforce it at build time
- Update dependencies

## 0.1.9 - 2024-10-09
- Fix broken wallpapers on 90 degree rotated monitors [#9](https://github.com/gergo-salyi/multibg-sway/issues/9)
- Update dependencies

## 0.1.8 - 2024-09-30
- Try to fix crash with wayland protocol error regarding wlr_layer_surface [#8](https://github.com/gergo-salyi/multibg-sway/issues/8)
- Update dependencies
- Add logging messages
- Code formatting with editorconfig

## 0.1.7 - 2024-05-11
- Fix image corruption for certain pixel formats when output width is not a multiple of 4 [#6](https://github.com/gergo-salyi/multibg-sway/issues/6)
- Add the --pixelformat cli argument. Setting --pixelformat=baseline can force wl_buffers to use the wayland default xrgb8888 pixel format if bgr888 or future others would break for any reason

## 0.1.6 - 2024-03-25
- Fix displaying the wallpapers on outputs with fractional scale factor. This may help with [#5](https://github.com/gergo-salyi/multibg-sway/issues/5)

## 0.1.5 - 2024-01-02
- Fix displaying the wallpapers on outputs with higher than 1 integer scale factor. This may help with [#4](https://github.com/gergo-salyi/multibg-sway/issues/4)

## 0.1.4 - 2023-08-31
- Allocate/release graphics memory per output when the output is connected/disconnected. This may help with [#2](https://github.com/gergo-salyi/multibg-sway/issues/2)
- Log graphics memory use (our wayland shared memory pool sizes)
- Minor fix to avoid a logged error on redrawing backgrounds already being drawn
- Update dependencies

## 0.1.3 - 2023-05-05
- Update dependencies, including fast_image_resize which fixed a major bug

## 0.1.2 - 2023-04-27
- Fix crash on suspend [#1](https://github.com/gergo-salyi/multibg-sway/issues/1)
- Implement automatic image resizing

## 0.1.1
- Initial release
