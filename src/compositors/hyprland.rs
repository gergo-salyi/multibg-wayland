// https://wiki.hyprland.org/IPC/

use super::*;

pub struct Hyprland {
    hyprctl_sock_path: Box<Path>,
    event_sock_path: Box<Path>,
    subscriber: Option<Subscriber>,
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
            subscriber: None,
        }
    }

    fn connect_hyprctl_sock(&self) -> HyprctlConn {
        HyprctlConn::connect(&self.hyprctl_sock_path)
    }
}

impl CompositorCtx for Hyprland {
    fn list_outputs(&mut self) {
        let mut buf = Vec::new();
        let monitors = self.connect_hyprctl_sock().monitors(&mut buf);
        let mut stdout_lock = stdout().lock();
        for monitor in monitors {
            print_output(
                &mut stdout_lock,
                &monitor.name,
                &monitor.make,
                &monitor.model,
                &monitor.serial,
            ).unwrap();
        }
    }

    fn set_wallpaper(&mut self, bg_layer: &mut BgLayer) {
        let mut buf = Vec::new();
        let monitors = self.connect_hyprctl_sock().monitors(&mut buf);
        if let Some(monitor) = monitors.into_iter()
            .find(|monitor| *monitor.name == *bg_layer.output_name)
        {
            bg_layer.set_wallpaper(
                &monitor.active_workspace.name,
                monitor.active_workspace.id.unwrap_or(-1),
            );
        }
    }

    fn set_all_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        let mut buf = Vec::new();
        let monitors = self.connect_hyprctl_sock().monitors(&mut buf);
        monitors.into_iter().for_each(|monitor|
            set_wallpaper(
                bg_layers,
                &monitor.name,
                &monitor.active_workspace.name,
                monitor.active_workspace.id.unwrap_or(-1),
            )
        )
    }

    fn make_model_serial(&mut self, output: &str) -> Option<Box<str>> {
        let mut buf = Vec::new();
        let monitors = self.connect_hyprctl_sock().monitors(&mut buf);
        let monitor = monitors.into_iter()
            .find(|monitor| monitor.name == output).unwrap();
        make_model_serial(&monitor.make, &monitor.model, &monitor.serial)
    }

    fn subscribe(&mut self) -> BorrowedFd<'_> {
        assert!(self.subscriber.is_none());
        let subscriber = Subscriber::new(self);
        self.subscriber.insert(subscriber).buf_sock.as_fd()
    }

    fn read_set_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        self.subscriber.as_mut().unwrap().read_set_wallpapers(bg_layers)
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
                Ok(_) | Err(Errno::INTR) => (),
                Err(e) => panic!("Failed to read Hyprland IPC socket: {e}"),
            }
        }
        serde_json::from_slice(buf)
            .expect("Failed to parse Hyprland monitors response")
    }
}

struct Subscriber {
    buf_sock: BufSock,
    active_monitor: String,
    has_workspacev2: bool,
    hyprctl_sock_path: Box<Path>,
}

impl Subscriber {
    fn new(ctx: &Hyprland) -> Self {
        let buf_sock = BufSock::connect(&ctx.event_sock_path).unwrap();
        let active_monitor = ctx.connect_hyprctl_sock()
            .monitors(&mut Vec::new()).into_iter()
            .find(|monitor| monitor.focused).unwrap()
            .name.into_owned();
        Self {
            buf_sock,
            active_monitor,
            has_workspacev2: false,
            hyprctl_sock_path: ctx.hyprctl_sock_path.clone(),
        }
    }

    fn read_set_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        self.buf_sock.recv_nonblock().unwrap();
        while let Some(event) = next_event(&mut self.buf_sock.read_buf) {
            if !self.has_workspacev2 && event.name == b"workspace" {
                set_wallpaper(
                    bg_layers,
                    &self.active_monitor,
                    str::from_utf8(event.data).unwrap(),
                    -1,
                );
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
                let workspace_name = str::from_utf8(
                    &event.data[comma_pos + 1..]
                ).unwrap();
                set_wallpaper(
                    bg_layers,
                    &self.active_monitor,
                    workspace_name,
                    workspace_number,
                );
            } else if event.name == b"focusedmon" {
                let comma_pos = memchr(event.data, b',').unwrap();
                let monname = &event.data[..comma_pos];
                if self.active_monitor.as_bytes() != monname {
                    self.active_monitor.clear();
                    self.active_monitor.push_str(
                        str::from_utf8(monname).unwrap()
                    );
                }
            } else if event.name == b"moveworkspace"
                || event.name == b"renameworkspace"
            {
                let mut buf = Vec::new();
                let monitors = HyprctlConn::connect(&self.hyprctl_sock_path)
                    .monitors(&mut buf);
                for monitor in monitors {
                    if monitor.focused
                        && self.active_monitor != monitor.name.as_ref()
                    {
                        self.active_monitor.clear();
                        self.active_monitor.push_str(monitor.name.as_ref());
                    }
                    set_wallpaper(
                        bg_layers,
                        monitor.name.as_ref(),
                        monitor.active_workspace.name.as_ref(),
                        monitor.active_workspace.id.unwrap_or(-1),
                    );
                }
            }
        }
    }
}

fn next_event(read_buf: &mut ReadBuf) -> Option<Event<'_>> {
    let buf = &read_buf.buf;
    let mut parsed_len = read_buf.parsed_len;

    let idx = memmem(&buf[parsed_len..], b">>")?;
    let name = &buf[parsed_len..][..idx];
    parsed_len += idx + 2;

    let idx = memchr(&buf[parsed_len..], b'\n')?;
    let data = &buf[parsed_len..][..idx];
    parsed_len += idx + 1;

    read_buf.parsed_len = parsed_len;
    Some(Event { name, data })
}

#[derive(Deserialize)]
struct Monitor<'a> {
    #[serde(borrow)]
    name: Cow<'a, str>,
    #[serde(borrow)]
    make: Cow<'a, str>,
    #[serde(borrow)]
    model: Cow<'a, str>,
    #[serde(borrow)]
    serial: Cow<'a, str>,
    #[serde(borrow, rename = "activeWorkspace")]
    active_workspace: ActiveWorkspace<'a>,
    focused: bool,
}

#[derive(Deserialize)]
struct ActiveWorkspace<'a> {
    id: Option<i32>,
    #[serde(borrow)]
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
