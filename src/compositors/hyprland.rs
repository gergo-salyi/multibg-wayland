// https://wiki.hyprland.org/IPC/

use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
};

use log::debug;
use serde::Deserialize;

use super::{
    CompositorInterface,
    EventSender,
    make_model_serial,
    OutputInfo,
    WorkspaceVisible,
};

pub struct HyprlandConnectionTask {}

impl HyprlandConnectionTask {
    pub fn new() -> Self {
        HyprlandConnectionTask {}
    }
}

impl CompositorInterface for HyprlandConnectionTask {
    fn request_visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        current_state().visible_workspaces
    }

    fn request_outputs(&mut self) -> Vec<OutputInfo> {
        monitors().into_iter()
            .map(|monitor| OutputInfo {
                name: monitor.name,
                make_model_serial: make_model_serial(
                    &monitor.make,
                    &monitor.model,
                    &monitor.serial,
                )
            })
            .collect()
    }

    fn subscribe_event_loop(self, event_sender: EventSender) {
        let mut socket = socket_dir_path();
        socket.push(".socket2.sock");
        let mut connection = UnixStream::connect(socket)
            .expect("Failed to connect to Hyprland events socket");
        let initial_state = current_state();
        for workspace in initial_state.visible_workspaces {
            event_sender.send(workspace);
        }
        let mut active_monitor = initial_state.active_monitor;
        let mut buf = vec![0u8; 2000];
        let mut filled = 0usize;
        let mut parsed = 0usize;
        loop {
            let read = connection.read(&mut buf[filled..]).unwrap();
            if read == 0 {
                panic!("Hyperland events socket disconnected");
            }
            filled += read;
            if filled == buf.len() {
                let new_len = buf.len() * 2;
                debug!("Growing Hyprland socket read buffer to {new_len}");
                buf.resize(new_len, 0u8);
            }
            loop {
                let mut unparsed = &buf[parsed..filled];
                let Some(gt_pos) = unparsed.iter().position(|&b| b == b'>')
                else { break };
                let event_name = &unparsed[..gt_pos];
                unparsed = &unparsed[gt_pos+2..];
                let Some(lf_pos) = unparsed.iter().position(|&b| b == b'\n')
                else { break };
                let event_data = &unparsed[..lf_pos];
                unparsed = &unparsed[lf_pos+1..];
                parsed = filled - unparsed.len();
                debug!(
                    "Hyprland event: {} {}",
                    String::from_utf8_lossy(event_name),
                    String::from_utf8_lossy(event_data),
                );
                if event_name == b"workspace" {
                    event_sender.send(WorkspaceVisible {
                        output: active_monitor.clone(),
                        workspace_name: String::from_utf8(event_data.to_vec())
                            .unwrap(),
                        workspace_number: -1,
                    });
                } else if event_name == b"focusedmon" {
                    let comma_pos = event_data.iter()
                        .position(|&b| b == b',').unwrap();
                    let monname = &event_data[..comma_pos];
                    active_monitor = String::from_utf8(monname.to_vec())
                        .unwrap();
                } else if event_name == b"moveworkspace"
                    || event_name == b"renameworkspace"
                {
                    let current_state = current_state();
                    for workspace in current_state.visible_workspaces {
                        event_sender.send(workspace);
                    }
                    active_monitor = current_state.active_monitor;
                }
            }
            if parsed == filled {
                filled = 0;
                parsed = 0;
            } else {
                buf.copy_within(parsed..filled, 0);
                filled -= parsed;
                parsed = 0;
            }
        }
    }
}

// "$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE"
fn socket_dir_path() -> PathBuf {
    let xdg_runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .expect("Environment variable XDG_RUNTIME_DIR not set");
    let his = env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .expect("Environment variable HYPRLAND_INSTANCE_SIGNATURE not set");
    let mut ret = PathBuf::with_capacity(256);
    ret.push(xdg_runtime_dir);
    ret.push("hypr");
    ret.push(his);
    ret
}

fn current_state() -> CurrentState {
    let mut active_monitor = String::new();
    let mut visible_workspaces = Vec::new();
    for monitor in monitors() {
        if monitor.focused {
            active_monitor = monitor.name.clone();
        }
        visible_workspaces.push(WorkspaceVisible {
            output: monitor.name,
            workspace_name: monitor.active_workspace.name,
            workspace_number: -1,
        });
    }
    CurrentState { active_monitor, visible_workspaces }
}

fn monitors() -> Vec<Monitor> {
    let mut socket = socket_dir_path();
    socket.push(".socket.sock");
    let mut connection = UnixStream::connect(socket)
        .expect("Failed to connect to Hyprland requests socket");
    connection.write_all(b"j/monitors")
        .expect("Failed to send Hyprland monitors requests");
    let mut buf = Vec::with_capacity(2000);
    // This socket .socket.sock for hyprctl-like requests
    // only allows one round trip with a single or batched commands
    let read = connection.read_to_end(&mut buf)
        .expect("Failed to receive Hyprland monitors response");
    serde_json::from_slice(&buf[..read])
        .expect("Failed to parse Hyprland monitors response")
}

struct CurrentState {
    active_monitor: String,
    visible_workspaces: Vec<WorkspaceVisible>,
}

#[derive(Deserialize)]
struct Monitor {
    name: String,
    make: String,
    model: String,
    serial: String,
    #[serde(rename = "activeWorkspace")]
    active_workspace: ActiveWorkspace,
    focused: bool,
}

#[derive(Deserialize)]
struct ActiveWorkspace {
    name: String,
}
