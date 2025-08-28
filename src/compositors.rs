mod hyprland;
mod niri2502;
mod niri2505;
mod sway;

use std::{
    env,
    os::unix::ffi::OsStrExt,
    process::Command,
    sync::{mpsc::Sender, Arc},
    thread,
};

use anyhow::{bail, Context};
use serde::Deserialize;
use log::{debug, warn};

use crate::poll::Waker;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Compositor {
    Hyprland,
    Niri,
    Sway,
}

pub struct OutputInfo {
    pub name: String,
    pub make_model_serial: String,
}

impl Compositor {
    pub fn from_env() -> Option<Compositor> {
        Compositor::from_xdg_desktop_var("XDG_SESSION_DESKTOP")
            .or_else(|| Compositor::from_xdg_desktop_var("XDG_CURRENT_DESKTOP"))
            .or_else(Compositor::from_ipc_socket_var)
    }

    fn from_xdg_desktop_var(xdg_desktop_var: &str) -> Option<Compositor> {
        if let Some(xdg_desktop) = env::var_os(xdg_desktop_var) {
            if xdg_desktop.as_bytes().starts_with(b"sway") {
                debug!("Selecting compositor Sway based on {xdg_desktop_var}");
                Some(Compositor::Sway)
            } else if xdg_desktop.as_bytes().starts_with(b"Hyprland") {
                debug!("Selecting compositor Hyprland based on {}",
                    xdg_desktop_var);
                Some(Compositor::Hyprland)
            } else if xdg_desktop.as_bytes().starts_with(b"niri") {
                debug!("Selecting compositor Niri based on {xdg_desktop_var}");
                Some(Compositor::Niri)
            } else {
                warn!("Unrecognized compositor from {xdg_desktop_var} \
                    environment variable: {xdg_desktop:?}");
                None
            }
        } else {
            None
        }
    }

    fn from_ipc_socket_var() -> Option<Compositor> {
        if env::var_os("SWAYSOCK").is_some() {
            debug!("Selecting compositor Sway based on SWAYSOCK");
            Some(Compositor::Sway)
        } else if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            debug!("Selecting compositor Hyprland based on \
                HYPRLAND_INSTANCE_SIGNATURE");
            Some(Compositor::Hyprland)
        } else if env::var_os("NIRI_SOCKET").is_some() {
            debug!("Selecting compositor Niri based on NIRI_SOCKET");
            Some(Compositor::Niri)
        } else {
            None
        }
    }

    pub fn list_outputs(&self) -> Vec<OutputInfo> {
        match self {
            Compositor::Sway =>
                sway::SwayConnectionTask::new().request_outputs(),
            Compositor::Hyprland =>
                hyprland::HyprlandConnectionTask::new().request_outputs(),
            Compositor::Niri => match get_niri_version() {
                Ok(niri_verison) => if niri_verison >= niri_ver(25, 5) {
                    niri2505::NiriConnectionTask::new().request_outputs()
                } else {
                    niri2502::NiriConnectionTask::new().request_outputs()
                },
                Err(e) => {
                    warn!("Failed to get niri version: {e:#}");
                    niri2505::NiriConnectionTask::new().request_outputs()
                }
            }
        }
    }
}

// impl From<&str> for Compositor {
//     fn from(s: &str) -> Self {
//         match s {
//             "sway" => Compositor::Sway,
//             "niri" => Compositor::Niri,
//             _ => panic!("Unknown compositor"),
//         }
//     }
// }

/// abstract 'sending back workspace change events'
struct EventSender {
    tx: Sender<WorkspaceVisible>,
    waker: Arc<Waker>,
}

impl EventSender {
    fn new(tx: Sender<WorkspaceVisible>, waker: Arc<Waker>) -> Self {
        EventSender { tx, waker }
    }

    fn send(&self, workspace: WorkspaceVisible) {
        self.tx.send(workspace).unwrap();
        self.waker.wake();
    }
}

trait CompositorInterface: Send + Sync {
    fn request_visible_workspaces(&mut self) -> Vec<WorkspaceVisible>;
    fn request_outputs(&mut self) -> Vec<OutputInfo>;
    fn subscribe_event_loop(self, event_sender: EventSender);
}

pub struct ConnectionTask {
    tx: Sender<WorkspaceVisible>,
    waker: Arc<Waker>,
    interface: Box<dyn CompositorInterface>,
}

impl ConnectionTask {
    pub fn new(
        composer: Compositor,
        tx: Sender<WorkspaceVisible>,
        waker: Arc<Waker>,
    ) -> Self {
        let interface: Box<dyn CompositorInterface> = match composer {
            Compositor::Sway => Box::new(sway::SwayConnectionTask::new()),
            Compositor::Hyprland => Box::new(
                hyprland::HyprlandConnectionTask::new()
            ),
            Compositor::Niri => match get_niri_version() {
                Ok(niri_verison) => if niri_verison >= niri_ver(25, 5) {
                    Box::new(niri2505::NiriConnectionTask::new())
                } else {
                    Box::new(niri2502::NiriConnectionTask::new())
                },
                Err(e) => {
                    warn!("Failed to get niri version: {e:#}");
                    Box::new(niri2505::NiriConnectionTask::new())
                }
            }
        };

        ConnectionTask {
            tx,
            waker,
            interface,
        }
    }

    pub fn spawn_subscribe_event_loop(
        composer: Compositor,
        tx: Sender<WorkspaceVisible>,
        waker: Arc<Waker>,
    ) {
        let event_sender = EventSender::new(tx, waker);
        thread::Builder::new()
            .name("compositor".to_string())
            .spawn(move || match composer {
                Compositor::Sway => {
                    let composer_interface = sway::SwayConnectionTask::new();
                    composer_interface.subscribe_event_loop(event_sender);
                }
                Compositor::Hyprland => {
                    let composer_interface =
                        hyprland::HyprlandConnectionTask::new();
                    composer_interface.subscribe_event_loop(event_sender);
                }
                Compositor::Niri => match get_niri_version() {
                    Ok(niri_verison) => if niri_verison >= niri_ver(25, 5) {
                        niri2505::NiriConnectionTask::new()
                            .subscribe_event_loop(event_sender)
                    } else {
                        niri2502::NiriConnectionTask::new()
                            .subscribe_event_loop(event_sender)
                    },
                    Err(e) => {
                        warn!("Failed to get niri version: {e:#}");
                        niri2505::NiriConnectionTask::new()
                            .subscribe_event_loop(event_sender)
                    }
                }
            })
            .unwrap();
    }

    pub fn request_visible_workspace(&mut self, output: &str) {
        if let Some(workspace) = self
            .interface
            .request_visible_workspaces()
            .into_iter()
            .find(|w| w.output == output)
        {
            self.tx
                .send(WorkspaceVisible {
                    output: workspace.output,
                    workspace_name: workspace.workspace_name,
                    workspace_number: workspace.workspace_number,
                })
                .unwrap();

            self.waker.wake();
        }
    }

    pub fn request_visible_workspaces(&mut self) {
        for workspace in self.interface
            .request_visible_workspaces().into_iter()
        {
            self.tx
                .send(WorkspaceVisible {
                    output: workspace.output,
                    workspace_name: workspace.workspace_name,
                    workspace_number: workspace.workspace_number
                })
                .unwrap();

            self.waker.wake();
        }
    }

    pub fn request_make_model_serial(&mut self, output_name: &str) -> String {
        self.interface.request_outputs().into_iter()
            .find(|output_info| output_info.name == output_name)
            .map(|output_info| output_info.make_model_serial)
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub struct WorkspaceVisible {
    pub output: String,
    pub workspace_name: String,
    pub workspace_number: i32,
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

fn make_model_serial(make: &str, model: &str, serial: &str) -> String {
    [make, model, serial].into_iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<&str>>()
        .join(" ")
}
