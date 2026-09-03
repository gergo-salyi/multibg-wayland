mod hyprland;
mod niri;
mod sway;

use super::*;

use rustix::{
    buffer::spare_capacity,
    io::{Errno, read},
    net::{
        AddressFamily, connect, send, SendFlags,
        socket_with, SocketAddrUnix, SocketFlags, SocketType,
    },
};
use serde::{Deserialize, de::IgnoredAny};

#[derive(Clone, Copy, Debug)]
pub enum Compositor {
    Hyprland,
    Niri,
    Sway,
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
                debug_selecting("Sway", xdg_desktop_var);
                Some(Compositor::Sway)
            } else if xdg_desktop.as_bytes().starts_with(b"Hyprland") {
                debug_selecting("Hyprland", xdg_desktop_var);
                Some(Compositor::Hyprland)
            } else if xdg_desktop.as_bytes().starts_with(b"niri") {
                debug_selecting("niri", xdg_desktop_var);
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
            debug_selecting("Sway", "SWAYSOCK");
            Some(Compositor::Sway)
        } else if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            debug_selecting("Hyprland", "HYPRLAND_INSTANCE_SIGNATURE");
            Some(Compositor::Hyprland)
        } else if env::var_os("NIRI_SOCKET").is_some() {
            debug_selecting("niri", "NIRI_SOCKET");
            Some(Compositor::Niri)
        } else {
            None
        }
    }

    pub fn context(self) -> Box<dyn CompositorCtx> {
        match self {
            Compositor::Sway => Box::new(sway::Sway::new()),
            Compositor::Hyprland => Box::new(hyprland::Hyprland::new()),
            Compositor::Niri => Box::new(niri::Niri::new()),
        }
    }
}

pub trait CompositorCtx {
    fn list_outputs(&mut self);
    fn visible_workspaces(&mut self) -> Vec<WorkspaceVisible>;
    fn make_model_serial(&mut self, output: &str) -> String;
    fn subscribe(&mut self, event_sender: EventSender);

    fn visible_workspace(&mut self, output: &str) -> Option<WorkspaceVisible> {
        self.visible_workspaces().into_iter().find(|w| w.output == output)
    }
}

/// abstract 'sending back workspace change events'
pub struct EventSender {
    tx: Sender<WorkspaceVisible>,
    waker: Arc<Waker>,
}

impl EventSender {
    pub fn new(tx: Sender<WorkspaceVisible>, waker: Arc<Waker>) -> Self {
        EventSender { tx, waker }
    }

    fn send(&self, workspace: WorkspaceVisible) {
        self.tx.send(workspace).unwrap();
        self.waker.wake();
    }
}

#[derive(Debug)]
pub struct WorkspaceVisible {
    pub output: String,
    pub workspace_name: String,
    pub workspace_number: i32,
}

fn make_model_serial(make: &str, model: &str, serial: &str) -> String {
    [make, model, serial].into_iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<&str>>()
        .join(" ")
}

fn debug_selecting(compositor: &str, reason: &str) {
    debug!("Selecting compositor {compositor} based on {reason}");
}

fn connect_unix(path: &Path) -> Option<OwnedFd> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    ).ok()?;
    connect(
        &socket,
        &SocketAddrUnix::new(path).ok()?,
    ).ok()?;
    Some(socket)
}
