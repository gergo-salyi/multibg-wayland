// https://wiki.hyprland.org/IPC/

use super::*;

pub struct Hyprland {
    hyprctl_sock_path: Box<Path>,
    event_sock_path: Box<Path>,
    subscribe_task: Option<JoinHandle<()>>,
}

impl Hyprland {
    pub fn new() -> Self {
        let xdg_runtime_dir = env::var_os("XDG_RUNTIME_DIR")
            .expect("Environment variable XDG_RUNTIME_DIR not set");
        let signature = env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
            .expect("Environment variable HYPRLAND_INSTANCE_SIGNATURE not set");
        let hyprctl_sock_path = Box::from(Path::new(&concat_os_str(&[
            &xdg_runtime_dir,
            OsStr::new("/hypr/"),
            &signature,
            OsStr::new("/.socket.sock"),
        ])));
        let event_sock_path = Box::from(Path::new(&concat_os_str(&[
            &xdg_runtime_dir,
            OsStr::new("/hypr/"),
            &signature,
            OsStr::new("/.socket2.sock"),
        ])));
        Self {
            hyprctl_sock_path,
            event_sock_path,
            subscribe_task: None,
        }
    }

    fn connect_hyprctl_sock(&self) -> HyprctlConn {
        HyprctlConn::connect(&self.hyprctl_sock_path)
    }

    fn subscriber(&self, event_sender: EventSender) -> Subscriber {
        Subscriber::new(self, event_sender)
    }
}

impl CompositorCtx for Hyprland {
    fn list_outputs(&mut self) {
        let mut buf = Vec::new();
        let monitors = self.connect_hyprctl_sock().monitors(&mut buf);
        let mut stdout = stdout().lock();
        for monitor in monitors {
            writeln!(
                stdout,
                "{}: '{}'",
                monitor.name,
                make_model_serial(
                    &monitor.make,
                    &monitor.model,
                    &monitor.serial,
                ),
            ).unwrap();
        }
    }

    fn visible_workspace(&mut self, output: &str) -> Option<WorkspaceVisible> {
        self.connect_hyprctl_sock().visible_workspace(output)
    }

    fn visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        self.connect_hyprctl_sock().visible_workspaces()
    }

    fn make_model_serial(&mut self, output: &str) -> String {
        self.connect_hyprctl_sock().make_model_serial(output)
    }

    fn subscribe(&mut self, event_sender: EventSender) {
        assert!(self.subscribe_task.is_none());
        let join_handle = self.subscriber(event_sender).spawn();
        self.subscribe_task = Some(join_handle);
    }
}

// This socket .socket.sock for hyprctl-like requests
// only allows one round trip with a single or batched commands
struct HyprctlConn {
    socket: OwnedFd,
}

impl HyprctlConn {
    fn connect(path: &Path) -> Self {
        let socket = connect_unix(path).unwrap();
        Self { socket }
    }

    fn visible_workspace(self, output: &str) -> Option<WorkspaceVisible> {
        let mut buf = Vec::new();
        let monitors = self.monitors(&mut buf);
        monitors.into_iter()
            .find(|monitor| monitor.name == output)
            .map(Monitor::into_visible_workspace)
    }

    fn visible_workspaces(self) -> Vec<WorkspaceVisible> {
        let mut buf = Vec::new();
        let monitors = self.monitors(&mut buf);
        monitors.into_iter()
            .map(Monitor::into_visible_workspace)
            .collect()
    }

    fn make_model_serial(self, output: &str) -> String {
        let mut buf = Vec::new();
        let monitors = self.monitors(&mut buf);
        let monitor = monitors.into_iter()
            .find(|monitor| monitor.name == output).unwrap();
        make_model_serial(&monitor.make, &monitor.model, &monitor.serial)
    }

    fn monitors(self, buf: &mut Vec<u8>) -> Vec<Monitor<'_>> {
        let mut request = b"j/monitors".as_slice();
        while !request.is_empty() {
            match send(&self.socket, request, SendFlags::NOSIGNAL) {
                Ok(0) => panic!(),
                Ok(count) => request = &request[count..],
                Err(Errno::INTR) => (),
                Err(Errno::PIPE) => panic!("Hyprland IPC socket disconnected"),
                Err(e) => panic!("Failed to send to Hyprland IPC socket: {e}"),
            }
        }
        buf.clear();
        buf.reserve(2000);
        loop {
            if buf.len() == buf.capacity() {
                buf.reserve(buf.capacity());
            }
            match read(&self.socket, spare_capacity(buf)) {
                Ok(0) => break,
                Ok(_) => (),
                Err(Errno::INTR) => (),
                Err(e) => panic!("Failed to read Hyprland IPC socket: {e}"),
            }
        }
        serde_json::from_slice(buf)
            .expect("Failed to parse Hyprland monitors response")
    }
}

struct Subscriber {
    conn: EventConn,
    event_sender: EventSender,
    active_monitor: Box<str>,
    has_workspacev2: bool,
    hyprctl_sock_path: Box<Path>,
}

impl Subscriber {
    fn new(ctx: &Hyprland, event_sender: EventSender) -> Self {
        let conn = EventConn::connect(&ctx.event_sock_path);
        let active_monitor = ctx.connect_hyprctl_sock()
            .monitors(&mut Vec::new()).into_iter()
            .find(|monitor| monitor.focused).unwrap()
            .name.into_owned().into_boxed_str();
        Self {
            conn,
            event_sender,
            active_monitor,
            has_workspacev2: false,
            hyprctl_sock_path: ctx.hyprctl_sock_path.clone(),
        }
    }

    fn spawn(mut self) -> JoinHandle<()> {
        thread::Builder::new()
            .name(String::from("hyprland-conn"))
            .spawn(move || loop {
                self.cycle();
            }).unwrap()
    }

    fn cycle(&mut self) {
        let event = self.conn.recv();
        if !self.has_workspacev2 && event.name == b"workspace" {
            self.event_sender.send(WorkspaceVisible {
                output: String::from(self.active_monitor.as_ref()),
                workspace_name: String::from_utf8(event.data.to_vec()).unwrap(),
                workspace_number: -1,
            });
        } else if event.name == b"workspacev2" {
            // This is a bit hacky to detect support for "workspacev2"
            // events here, as we might got "workspace" and sent a
            // { workspace_number: -1 } before we got here.
            // But it shouldn't really matter, it will get corrected
            // by sending a duplicate now and setting has_workspacev2
            self.has_workspacev2 = true;
            let comma_pos = memchr(event.data, b',').unwrap();
            let workspace_number = str::from_utf8(
                &event.data[..comma_pos]
            ).unwrap().parse().unwrap();
            let workspace_name = String::from_utf8(
                event.data[comma_pos + 1..].to_vec()
            ).unwrap();
            self.event_sender.send(WorkspaceVisible {
                output: String::from(self.active_monitor.as_ref()),
                workspace_name,
                workspace_number,
            });
        } else if event.name == b"focusedmon" {
            let comma_pos = memchr(event.data, b',').unwrap();
            let monname = &event.data[..comma_pos];
            self.active_monitor = Box::from(
                str::from_utf8(monname).unwrap()
            );
        } else if event.name == b"moveworkspace"
            || event.name == b"renameworkspace"
        {
            let mut buf = Vec::new();
            let monitors = HyprctlConn::connect(&self.hyprctl_sock_path)
                .monitors(&mut buf);
            for monitor in monitors {
                if monitor.focused {
                    self.active_monitor = Box::from(monitor.name.as_ref());
                }
                self.event_sender.send(monitor.into_visible_workspace());
            }
        }
    }
}

struct EventConn {
    socket: OwnedFd,
    buf: Vec<u8>,
    parsed_len: usize,
}

impl EventConn {
    fn connect(path: &Path) -> Self {
        let socket = connect_unix(path).unwrap();
        Self {
            socket,
            buf: Vec::with_capacity(240),
            parsed_len: 0,
        }
    }

    fn recv(&mut self) -> Event<'_> {
        loop {
            if let Some(event) = self.take_event() {
                return event
            }
            self.prepare_buf();
            match read(&self.socket, spare_capacity(&mut self.buf)) {
                Ok(0) => panic!("Sway IPC socket disconnected"),
                Ok(_) => (),
                Err(Errno::INTR) => (),
                Err(e) => panic!("Failed to read Sway IPC socket: {e}"),
            }
        }
    }

    fn take_event(&mut self) -> Option<Event<'_>> {
        let mut parsed_len = self.parsed_len;

        let idx = memmem(&self.buf[parsed_len..], b">>")?;
        let name = &self.buf[parsed_len..][..idx];
        parsed_len += idx + 2;

        let idx = memchr(&self.buf[parsed_len..], b'\n')?;
        let data = &self.buf[parsed_len..][..idx];
        parsed_len += idx + 1;

        self.parsed_len = parsed_len;
        Some(Event { name, data })
    }

    fn prepare_buf(&mut self) {
        let unparsed_len = self.buf.len().strict_sub(self.parsed_len);
        if self.parsed_len != 0 {
            if unparsed_len == 0 {
                self.buf.clear();
            } else {
                self.buf.copy_within(self.parsed_len.., 0);
                self.buf.truncate(unparsed_len);
            }
            self.parsed_len = 0;
        } else if self.buf.len() == self.buf.capacity() {
            self.buf.reserve(self.buf.capacity() / 2);
        }
    }
}

#[derive(Deserialize)]
struct Monitor<'a> {
    name: Cow<'a, str>,
    make: Cow<'a, str>,
    model: Cow<'a, str>,
    serial: Cow<'a, str>,
    #[serde(rename = "activeWorkspace")]
    active_workspace: ActiveWorkspace<'a>,
    focused: bool,
}

impl Monitor<'_> {
    fn into_visible_workspace(self) -> WorkspaceVisible {
        WorkspaceVisible {
            output: self.name.into_owned(),
            workspace_name: self.active_workspace.name.into_owned(),
            workspace_number: self.active_workspace.id.unwrap_or(-1),
        }
    }
}

#[derive(Deserialize)]
struct ActiveWorkspace<'a> {
    id: Option<i32>,
    name: Cow<'a, str>,
}

fn concat_os_str(parts: &[&OsStr]) -> OsString {
    let mut os_string = OsString::with_capacity(
        parts.iter().map(|part| part.len()).sum()
    );
    for part in parts {
        os_string.push(part);
    }
    os_string
}

struct Event<'a> {
    name: &'a [u8],
    data: &'a [u8],
}
