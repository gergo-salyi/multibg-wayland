use super::{CompositorInterface, EventSender, WorkspaceVisible};
use swayipc::{Connection, Event, EventType, WorkspaceChange};

pub struct SwayConnectionTask {
    sway_conn: Connection,
}

impl SwayConnectionTask {
    pub fn new() -> Self {
        SwayConnectionTask {
            sway_conn: Connection::new().expect("Failed to connect to sway \
                socket. If you're not using sway, pass the correct \
                --compositor argument. Original cause"),
        }
    }
}

impl CompositorInterface for SwayConnectionTask {
    fn request_visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        self.sway_conn
            .get_workspaces()
            .unwrap()
            .into_iter()
            .filter(|w| w.visible)
            .map(|workspace| WorkspaceVisible {
                output: workspace.output,
                workspace_name: workspace.name,
            })
            .collect()
    }

    fn subscribe_event_loop(self, event_sender: EventSender) {
        let event_stream = self.sway_conn
            .subscribe([EventType::Workspace]).unwrap();
        for event_result in event_stream {
            let event = event_result.unwrap();
            let Event::Workspace(workspace_event) = event else {
                continue;
            };
            if let WorkspaceChange::Focus = workspace_event.change {
                let current_workspace = workspace_event.current.unwrap();
                event_sender.send(WorkspaceVisible {
                    output: current_workspace.output.unwrap(),
                    workspace_name: current_workspace.name.unwrap(),
                });
            }
        }
    }
}
