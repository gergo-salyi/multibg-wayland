mod hyprland;
mod niri;
mod sway;

use std::{
    env,
    os::unix::ffi::OsStrExt,
    sync::{mpsc::Sender, Arc},
    thread::spawn,
};

use log::{debug, warn};

use crate::poll::Waker;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
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
            Compositor::Niri => Box::new(niri::NiriConnectionTask::new()),
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
        spawn(move || match composer {
            Compositor::Sway => {
                let composer_interface = sway::SwayConnectionTask::new();
                composer_interface.subscribe_event_loop(event_sender);
            }
            Compositor::Hyprland => {
                let composer_interface =
                    hyprland::HyprlandConnectionTask::new();
                composer_interface.subscribe_event_loop(event_sender);
            }
            Compositor::Niri => {
                let composer_interface = niri::NiriConnectionTask::new();
                composer_interface.subscribe_event_loop(event_sender);
            }
        });
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
                })
                .unwrap();

            self.waker.wake();
        }
    }
}

#[derive(Debug)]
pub struct WorkspaceVisible {
    pub output: String,
    pub workspace_name: String,
}
