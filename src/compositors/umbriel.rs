use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    process::Command,
};

use log::debug;
use serde_json::Value;

use super::{
    CompositorInterface,
    EventSender,
    make_model_serial,
    OutputInfo,
    WorkspaceVisible,
};

pub struct UmbrielConnectionTask {}

impl UmbrielConnectionTask {
    pub fn new() -> Self {
        UmbrielConnectionTask {}
    }
}

impl CompositorInterface for UmbrielConnectionTask {
    fn request_visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        request_workspaces().iter()
            .filter(|w| w.get("focused").and_then(Value::as_bool)
                .unwrap_or(false))
            .map(workspace_visible)
            .collect()
    }

    fn request_outputs(&mut self) -> Vec<OutputInfo> {
        request_outputs()
    }

    fn subscribe_event_loop(self, event_sender: EventSender) {
        let mut stream = connect();
        stream.write_all(
            b"{\"cmd\":\"subscribe\",\"events\":[\"workspaces\"]}\n"
        ).expect("failed to send umbriel subscribe request");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line)
                .expect("failed to read umbriel event stream");
            if line.is_empty() {
                panic!("umbriel event stream closed");
            }
            let event: Value = serde_json::from_str(&line)
                .expect("failed to parse umbriel event");
            if event.get("event").and_then(Value::as_str) != Some("workspaces")
            {
                continue;
            }
            let Some(workspaces) = event.get("data").and_then(Value::as_array)
            else {
                continue;
            };
            for workspace in workspaces.iter()
                .filter(|w| w.get("focused").and_then(Value::as_bool)
                    .unwrap_or(false))
            {
                debug!("Umbriel event: workspace {} on {} focused",
                    workspace.get("name").and_then(Value::as_str)
                        .unwrap_or("?"),
                    workspace.get("output").and_then(Value::as_str)
                        .unwrap_or("?"));
                event_sender.send(workspace_visible(workspace));
            }
        }
    }
}

fn connect() -> UnixStream {
    let socket_path = env::var_os("UMBRIEL_SOCKET")
        .expect("UMBRIEL_SOCKET is not set. If you're not using umbriel, \
            pass the correct --compositor argument.");
    UnixStream::connect(&socket_path)
        .expect("failed to connect to umbriel socket")
}

fn request_workspaces() -> Vec<Value> {
    let mut stream = connect();
    stream.write_all(b"{\"cmd\":\"workspaces\"}\n")
        .expect("failed to send umbriel ipc request");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)
        .expect("failed to read umbriel ipc reply");
    let reply: Value = serde_json::from_str(&line)
        .expect("failed to parse umbriel ipc reply");
    if let Some(err) = reply.get("err") {
        panic!("umbriel ipc error: {err}");
    }
    reply.get("ok").and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn workspace_visible(workspace: &Value) -> WorkspaceVisible {
    WorkspaceVisible {
        output: workspace.get("output").and_then(Value::as_str)
            .unwrap_or_default().to_string(),
        workspace_name: workspace.get("name").and_then(Value::as_str)
            .unwrap_or_default().to_string(),
        workspace_number: workspace.get("index").and_then(Value::as_i64)
            .unwrap_or_default() as i32,
    }
}

fn request_outputs() -> Vec<OutputInfo> {
    let out = Command::new("umbriel")
        .args(["outputs"])
        .output()
        .expect("failed to run `umbriel outputs`");
    if !out.status.success() {
        panic!("`umbriel outputs` exited with {}: {}",
            out.status, String::from_utf8_lossy(&out.stderr));
    }
    parse_outputs(&String::from_utf8_lossy(&out.stdout))
}

fn parse_outputs(text: &str) -> Vec<OutputInfo> {
    let mut outputs: Vec<OutputInfo> = Vec::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            let name = line.split_whitespace().next()
                .unwrap_or_default().to_string();
            outputs.push(OutputInfo {
                name,
                make_model_serial: String::new(),
            });
        } else if let Some(output) = outputs.last_mut() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("Make:") {
                let (make, model, serial) =
                    parse_make_model_serial(trimmed);
                output.make_model_serial =
                    make_model_serial(&make, &model, &serial);
            }
        }
    }
    outputs
}

fn parse_make_model_serial(line: &str) -> (String, String, String) {
    let value = |key: &str| {
        let Some(pos) = line.find(&format!("{key}:")) else {
            return String::new();
        };
        let rest = &line[pos + key.len() + 1..];
        let end = ["Make", "Model", "Serial"].iter()
            .filter(|k| **k != key)
            .filter_map(|k| rest.find(&format!(" {k}:")))
            .min()
            .unwrap_or(rest.len());
        rest[..end].trim().to_string()
    };
    (value("Make"), value("Model"), value("Serial"))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::unix::net::UnixListener,
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Duration,
    };

    use crate::poll::Waker;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const WORKSPACES_REPLY: &str = concat!(
        "{\"ok\":[{\"active\":true,\"focused\":true,\"id\":\"HDMI-A-1:1\",",
        "\"index\":1,\"layout\":\"dwindle\",\"name\":\"1\",\"named\":false,",
        "\"occupied\":true,\"output\":\"HDMI-A-1\"},{\"active\":false,",
        "\"focused\":false,\"id\":\"HDMI-A-1:2\",\"index\":2,",
        "\"layout\":\"dwindle\",\"name\":\"2\",\"named\":false,",
        "\"occupied\":false,\"output\":\"HDMI-A-1\"}]}"
    );

    const WORKSPACES_EVENT: &str = concat!(
        "{\"event\":\"workspaces\",\"data\":[{\"active\":true,",
        "\"focused\":true,\"id\":\"HDMI-A-1:1\",\"index\":1,",
        "\"layout\":\"dwindle\",\"name\":\"1\",\"named\":false,",
        "\"occupied\":true,\"output\":\"HDMI-A-1\"},{\"active\":false,",
        "\"focused\":false,\"id\":\"HDMI-A-1:2\",\"index\":2,",
        "\"layout\":\"dwindle\",\"name\":\"2\",\"named\":false,",
        "\"occupied\":false,\"output\":\"HDMI-A-1\"}]}\n"
    );

    fn fake_server(socket_path: &str, reply: &'static str) {
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path).unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
            stream.write_all(b"\n").unwrap();
            // keep the connection open
            let mut buf = [0u8; 1];
            let _ = reader.read(&mut buf);
        });
    }

    #[test]
    fn request_workspaces_parses_fake_server() {
        let _guard = ENV_LOCK.lock().unwrap();
        let socket_path = "/tmp/opencode/fake-umbriel.sock";
        fake_server(socket_path, WORKSPACES_REPLY);
        env::set_var("UMBRIEL_SOCKET", socket_path);
        let visible = UmbrielConnectionTask::new()
            .request_visible_workspaces();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].output, "HDMI-A-1");
        assert_eq!(visible[0].workspace_name, "1");
        assert_eq!(visible[0].workspace_number, 1);
    }

    #[test]
    fn subscribe_event_loop_sends_focused_workspaces() {
        let _guard = ENV_LOCK.lock().unwrap();
        let socket_path = "/tmp/opencode/fake-umbriel-sub.sock";
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path).unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line,
                "{\"cmd\":\"subscribe\",\"events\":[\"workspaces\"]}\n");
            stream.write_all(WORKSPACES_EVENT.as_bytes()).unwrap();
            // keep the connection open
            let mut buf = [0u8; 1];
            let _ = reader.read(&mut buf);
        });
        env::set_var("UMBRIEL_SOCKET", socket_path);
        let (tx, rx) = mpsc::channel();
        let waker = Arc::new(Waker::new().unwrap());
        let event_sender = EventSender::new(tx, waker);
        thread::spawn(move || {
            UmbrielConnectionTask::new().subscribe_event_loop(event_sender);
        });
        let workspace = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(workspace.output, "HDMI-A-1");
        assert_eq!(workspace.workspace_name, "1");
        assert_eq!(workspace.workspace_number, 1);
    }

    #[test]
    fn parse_outputs_extracts_make_model_serial() {
        let text = concat!(
            "HDMI-A-1 \"WOR TERRA 2442W W539LZD00492 (HDMI-A-1)\"\n",
            "  Config name: \"WOR TERRA 2442W W539LZD00492\"\n",
            "  Make: WOR  Model: TERRA 2442W  Serial: W539LZD00492\n"
        );
        let outputs = parse_outputs(text);
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "HDMI-A-1");
        assert_eq!(outputs[0].make_model_serial,
            "WOR TERRA 2442W W539LZD00492");
    }
}