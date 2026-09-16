// https://docs.noctalia.dev/umbriel/ipc/

use super::*;

use std::process::Command;

pub struct Umbriel {
    umbriel_socket: Box<Path>,
    subscriber: Option<Connection>,
}

impl Umbriel {
    pub fn new() -> Self {
        Self {
            umbriel_socket: PathBuf::from(umbriel_socket()).into_boxed_path(),
            subscriber: None,
        }
    }

    fn connect(&self) -> Connection {
        Connection::connect(&self.umbriel_socket)
    }
}

impl CompositorCtx for Umbriel {
    fn list_outputs(&mut self) {
        let cmd_out = cmd_outputs();
        let outputs = parse_outputs(&cmd_out);
        let mut stdout_lock = stdout().lock();
        for output in outputs {
            print_output(
                &mut stdout_lock,
                &output.name,
                &output.make,
                &output.model,
                &output.serial,
            ).unwrap();
        }
    }

    fn set_wallpaper(&mut self, bg_layer: &mut BgLayer) {
        let mut conn = self.connect();
        if let Some(workspace) = conn.workspaces().into_iter()
            .find(|w| w.active && *w.output == *bg_layer.output_name)
        {
            bg_layer.set_wallpaper(&workspace.name, workspace.index);
        }
    }

    fn set_all_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        let mut conn = self.connect();
        let workspaces = conn.workspaces();
        for workspace in workspaces.into_iter().filter(|w| w.active) {
            set_wallpaper(
                bg_layers,
                &workspace.output,
                &workspace.name,
                workspace.index,
            )
        }
    }

    fn make_model_serial(&mut self, output: &str) -> Option<Box<str>> {
        let cmd_out = cmd_outputs();
        let output = parse_outputs(&cmd_out).into_iter()
            .find(|o| o.name == output)?;
        make_model_serial(
            &output.make,
            &output.model,
            &output.serial,
        )
    }

    fn subscribe(&mut self) -> BorrowedFd<'_> {
        assert!(self.subscriber.is_none());
        let mut conn = self.connect();
        conn.subscribe();
        self.subscriber.insert(conn).buf_sock.as_fd()
    }

    fn read_set_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        self.subscriber.as_mut().unwrap().read_set_wallpapers(bg_layers)
    }
}

struct Connection {
    buf_sock: BufSock,
}

impl Connection {
    fn connect(umbriel_socket: &Path) -> Self {
        Self { buf_sock: BufSock::connect(umbriel_socket).unwrap() }
    }

    fn workspaces(&mut self) -> Vec<Workspace<'_>> {
        self.buf_sock.send_all(b"{\"cmd\":\"workspaces\"}\n").unwrap();
        loop {
            self.buf_sock.recv_blocking().unwrap();
            if let Some(line) = self.buf_sock.read_buf.next_line() {
                return parse_workspaces(line)
            }
        }
    }

    fn subscribe(&mut self) {
        self.buf_sock.send_all(
            b"{\"cmd\":\"subscribe\",\"events\":[\"workspaces\"]}\n"
        ).unwrap();
    }

    fn read_set_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        self.buf_sock.recv_nonblock().unwrap();
        while let Some(line) = self.buf_sock.read_buf.next_line() {
            for workspace in parse_workspace_event(line).into_iter()
                .filter(|w| w.active)
            {
                set_wallpaper(
                    bg_layers,
                    &workspace.output,
                    &workspace.name,
                    workspace.index,
                )
            }
        }
    }
}

// $UMBRIEL_SOCKET or $XDG_RUNTIME_DIR/umbriel-$WAYLAND_DISPLAY.sock
fn umbriel_socket() -> OsString {
    if let Some(val) = env::var_os("UMBRIEL_SOCKET") {
        return val
    }
    concat_os_str(&[
        env::var_os("XDG_RUNTIME_DIR").unwrap().as_ref(),
        OsStr::new("/umbriel-"),
        env::var_os("WAYLAND_DISPLAY").unwrap().as_ref(),
        OsStr::new(".sock"),
    ])
}

fn parse_workspaces(line: &[u8]) -> Vec<Workspace<'_>> {
    let response: Workspaces = serde_json::from_slice(line).unwrap();
    if let Workspaces::Ok(workspaces) = response {
        workspaces
    } else {
        panic!()
    }
}

fn parse_workspace_event(line: &[u8]) -> Vec<Workspace<'_>> {
    let event: Event = serde_json::from_slice(line).unwrap();
    event.data
}

fn cmd_outputs() -> Vec<u8> {
    let out = Command::new("umbriel").args(["outputs", "--json"])
        .output().expect("failed to run `umbriel outputs`");
    if !out.status.success() {
        panic!("`umbriel outputs` exited with {}: {}",
            out.status, String::from_utf8_lossy(&out.stderr));
    }
    out.stdout
}

fn parse_outputs(cmd_out: &[u8]) -> Vec<Output<'_>> {
    serde_json::from_slice(cmd_out).unwrap()
}

#[derive(Deserialize)]
enum Workspaces<'a> {
    #[serde(borrow, rename = "ok")]
    Ok(Vec<Workspace<'a>>),
    #[serde(borrow, rename = "err")]
    Err(#[allow(dead_code)] Cow<'a, str>),
}

#[derive(Deserialize)]
struct Event<'a> {
    #[serde(borrow)]
    data: Vec<Workspace<'a>>
}

#[derive(Deserialize)]
struct Workspace<'a> {
    #[serde(borrow)]
    name: Cow<'a, str>,
    index: i32,
    #[serde(borrow)]
    output: Cow<'a, str>,
    active: bool,
}

#[derive(Deserialize)]
struct Output<'a> {
    #[serde(borrow)]
    name: Cow<'a, str>,
    #[serde(borrow)]
    make: Cow<'a, str>,
    #[serde(borrow)]
    model: Cow<'a, str>,
    #[serde(borrow)]
    serial: Cow<'a, str>,
}
