// https://niri-wm.github.io/niri/IPC.html#programmatic-access

use super::*;

pub struct Niri {
    niri_socket: Box<Path>,
    subscriber: Option<Subscriber>,
}

impl Niri {
    pub fn new() -> Self {
        let niri_socket = env::var_os("NIRI_SOCKET").unwrap();
        Self {
            niri_socket: PathBuf::from(niri_socket).into_boxed_path(),
            subscriber: None,
        }
    }

    fn connect(&self) -> Connection {
        Connection::connect(&self.niri_socket)
    }
}

impl CompositorCtx for Niri {
    fn list_outputs(&mut self) {
        let mut conn = self.connect();
        let outputs = conn.outputs();
        let mut stdout_lock = stdout().lock();
        outputs.into_iter().for_each(|(name, output)| {
            print_output(
                &mut stdout_lock,
                &name,
                &output.make,
                &output.model,
                output.serial.as_deref().unwrap_or(""),
            ).unwrap();
        });
    }

    fn set_wallpaper(&mut self, bg_layer: &mut BackgroundLayer) {
        let mut conn = self.connect();
        let workspaces = conn.workspaces();
        if let Some(workspace) = workspaces.into_iter()
            .find(|w| w.is_active
                && *w.output.as_ref().unwrap() == *bg_layer.output_name
            )
        {
            bg_layer.draw_workspace_bg(
                &workspace.name
                    .unwrap_or_else(|| format!("{}", workspace.idx).into()),
                workspace.idx.into(),
            );
        }
    }

    fn set_all_wallpapers(&mut self, bg_layers: &mut [BackgroundLayer]) {
        let mut conn = self.connect();
        let workspaces = conn.workspaces();
        workspaces.into_iter().filter(|w| w.is_active).for_each(|workspace|
            set_wallpaper(
                bg_layers,
                &workspace.output.unwrap(),
                &workspace.name
                    .unwrap_or_else(|| format!("{}", workspace.idx).into()),
                workspace.idx.into(),
            )
        )
    }

    fn make_model_serial(&mut self, output: &str) -> Option<Box<str>> {
        let mut conn = self.connect();
        let outputs = conn.outputs();
        let output = outputs.into_iter()
            .find(|(name, _)| name == output).unwrap()
            .1;
        make_model_serial(
            &output.make,
            &output.model,
            output.serial.as_deref().unwrap_or(""),
        )
    }

    fn subscribe(&mut self) -> BorrowedFd<'_> {
        assert!(self.subscriber.is_none());
        let subscriber = Subscriber::new(self);
        self.subscriber.insert(subscriber).conn.buf_sock.as_fd()
    }

    fn read_set_wallpapers(&mut self, bg_layers: &mut [BackgroundLayer]) {
        self.subscriber.as_mut().unwrap().read_set_wallpapers(bg_layers)
    }
}

const REQUEST_OUTPUTS: &[u8] = b"\"Outputs\"\n";
const REQUEST_WORKSPACES: &[u8] = b"\"Workspaces\"\n";
const REQUEST_EVENT_STREAM: &[u8] = b"\"EventStream\"\n";

struct Connection {
    buf_sock: BufSock,
}

impl Connection {
    fn connect(niri_socket: &Path) -> Self {
        Self { buf_sock: BufSock::connect(niri_socket).unwrap() }
    }

    fn send(&self, request: &[u8]) {
        self.buf_sock.send_all(request).unwrap();
    }

    fn recv_next_response_blocking(&mut self) -> Response<'_> {
        loop {
            match next_response(&mut self.buf_sock.read_buf) {
                Some(response) => return response,
                None => self.buf_sock.recv_blocking().unwrap(),
            }
        }
    }

    fn outputs(&mut self) -> HashMap<Cow<'_, str>, Output<'_>> {
        self.send(REQUEST_OUTPUTS);
        match self.recv_next_response_blocking() {
            Response::Outputs(outputs) => outputs,
            _ => panic!(),
        }
    }

    fn workspaces(&mut self) -> Vec<Workspace<'_>> {
        self.send(REQUEST_WORKSPACES);
        match self.recv_next_response_blocking() {
            Response::Workspaces(workspaces) => workspaces,
            _ => panic!(),
        }
    }

    fn event_stream(&mut self) {
        self.send(REQUEST_EVENT_STREAM);
        match self.recv_next_response_blocking() {
            Response::Handled => (),
            _ => panic!(),
        }
        while next_bytes(&mut self.buf_sock.read_buf).is_some() {}
    }
}

struct Subscriber {
    conn: Connection,
    workspaces: Vec<WorkspaceOwn>,
}

impl Subscriber {
    fn new(niri: &Niri) -> Self {
        let mut conn = niri.connect();
        conn.event_stream();
        let mut conn2 = niri.connect();
        let workspaces = conn2.workspaces().into_iter()
            .filter_map(WorkspaceOwn::from_workspace)
            .collect();
        Self { conn, workspaces }
    }

    fn read_set_wallpapers(&mut self, bg_layers: &mut [BackgroundLayer]) {
        self.conn.buf_sock.recv_nonblock().unwrap();
        while let Some(bytes) = next_bytes(&mut self.conn.buf_sock.read_buf) {
            let event: Event = serde_json::from_slice(bytes).unwrap();
            match event {
                Event::WorkspacesChanged { workspaces } => {
                    self.workspaces.clear();
                    self.workspaces.extend(
                        workspaces.into_iter()
                            .filter_map(WorkspaceOwn::from_workspace)
                    );
                },
                Event::WorkspaceActivated { id } => {
                    let workspace = self.workspaces.iter()
                        .find(|w| w.id == id).unwrap();
                    set_wallpaper(
                        bg_layers,
                        &workspace.output,
                        &workspace.name,
                        workspace.idx.into(),
                    );
                },
                Event::Other(_) => (),
            }
        }
    }
}

fn next_bytes(read_buf: &mut ReadBuf) -> Option<&[u8]> {
    let unparsed = &read_buf.buf[read_buf.parsed_len..];
    let idx = memchr(unparsed, b'\n')?;
    let reply_bytes = &unparsed[..idx];
    read_buf.parsed_len += idx + 1;
    Some(reply_bytes)
}

fn next_response(read_buf: &mut ReadBuf) -> Option<Response<'_>> {
    let reply_bytes = next_bytes(read_buf)?;
    let reply: Result<Response, String> =
        serde_json::from_slice(reply_bytes).unwrap();
    Some(reply.unwrap())
}

#[derive(Deserialize)]
enum Response<'a> {
    Handled,
    #[serde(borrow)]
    Outputs(HashMap<Cow<'a, str>, Output<'a>>),
    #[serde(borrow)]
    Workspaces(Vec<Workspace<'a>>),
}

#[derive(Deserialize)]
struct Output<'a> {
    #[serde(borrow)]
    make: Cow<'a, str>,
    #[serde(borrow)]
    model: Cow<'a, str>,
    #[serde(borrow)]
    serial: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Workspace<'a> {
    id: u64,
    idx: u8,
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow)]
    output: Option<Cow<'a, str>>,
    is_active: bool,
}

#[derive(Deserialize)]
enum Event<'a> {
    WorkspacesChanged {
        #[serde(borrow)]
        workspaces: Vec<Workspace<'a>>
    },
    WorkspaceActivated { id: u64 },
    #[serde(untagged)]
    Other(IgnoredAny),
}

struct WorkspaceOwn {
    id: u64,
    idx: u8,
    name: Box<str>,
    output: Box<str>,
}

impl WorkspaceOwn {
    fn from_workspace(workspace: Workspace) -> Option<Self> {
        let output = Box::from(workspace.output?);
        let name = workspace.name
            .map(Box::from)
            .unwrap_or_else(|| format!("{}", workspace.idx).into_boxed_str());
        Some(WorkspaceOwn {
            id: workspace.id,
            idx: workspace.idx,
            name,
            output,
        })
    }
}
