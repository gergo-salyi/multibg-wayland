mod niri2502;
mod niri2505;
mod niri2508;
mod niri2604;

use super::*;

use std::{
    process::Command,
};

pub fn context() -> Box<dyn CompositorCtx> {
    match get_niri_version() {
        Ok(niri_verison) => {
            if niri_verison >= niri_ver(25, 11) {
                Box::new(niri2604::Niri::new())
            } else if niri_verison >= niri_ver(25, 8) {
                Box::new(niri2508::Niri::new())
            } else if niri_verison >= niri_ver(25, 5) {
                Box::new(niri2505::Niri::new())
            } else {
                Box::new(niri2502::Niri::new())
            }
        },
        Err(e) => {
            warn!("Failed to get niri version: {e:#}");
            Box::new(niri2604::Niri::new())
        }
    }
}

#[derive(Deserialize)]
struct NiriVersionJson {
    compositor: String,
}

// Example:
// $ niri msg --json version
// {"cli":"25.02 (unknown commit)","compositor":"25.02 (unknown commit)"}
fn get_niri_version() -> anyhow::Result<u64> {
    let out = Command::new("niri")
        .args(["msg", "--json", "version"])
        .output().context("Command niri msg version failed")?;
    if !out.status.success() {
        bail!("Command niri msg version exited with {}: {}",
            out.status, String::from_utf8_lossy(&out.stderr));
    }
    let version_json: NiriVersionJson = serde_json::from_slice(&out.stdout)
        .context("Failed to deserialize niri msg version json")?;
    debug!("Niri version: {}", version_json.compositor);
    let version = parse_niri_version(&version_json.compositor)
        .context("Failed to parse niri version")?;
    Ok(version)
}

fn parse_niri_version(version_str: &str) -> Option<u64> {
    // Example: "25.02 (unknown commit)"
    let mut iter = version_str.split(|c: char| !c.is_ascii_digit());
    let major = iter.next()?.parse::<u32>().ok()?;
    let minor = iter.next()?.parse::<u32>().ok()?;
    Some(niri_ver(major, minor))
}

fn niri_ver(major: u32, minor: u32) -> u64 {
    ((major as u64) << 32) | (minor as u64)
}
