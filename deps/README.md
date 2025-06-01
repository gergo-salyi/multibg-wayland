# Vendored dependencies

We need to use multiple versions of the `niri-ipc` crate dependency to maintain compatibility with multiple version of niri. Ideally we would do this:

```toml
# Cargo.toml
[dependencies]
niri-ipc-25-2-0 = { package = "niri-ipc", version = "=25.2.0" }
niri-ipc-25-5-1 = { package = "niri-ipc", version = "=25.5.1" }
```

However for some braindamaged reasons `cargo` refuses to allow this if any of the multiple versions are semver compatible (see [cargo issue](https://github.com/rust-lang/cargo/issues/12787))

So we do a disgusting workaround here where we vendor older versions of the `niri-ipc` crate and re-publish them on crates.io under the name `multibg-wayland-niri-ipc` with semver incompatible versions e.g. `"25.2.0"` => `"0.250200.0"`

## License

Vendored dependencies are included here under their respective licenses

## Workflow

Example:

Download the crate:
```sh
curl --fail --proto '=https' --tlsv1.2 https://static.crates.io/crates/niri-ipc/niri-ipc-25.2.0.crate | tar -xz
```

Remove crates.io artifacts:
```sh
rm -f .cargo_vcs_info.json Cargo.toml.orig
```

Edit `Cargo.toml`:
- `name = "niri-ipc"` => `name = "multibg-wayland-niri-ipc"`
- `version = "25.2.0"` => `version = "0.250200.0"`

Re-publish:
```sh
cargo publish
```
