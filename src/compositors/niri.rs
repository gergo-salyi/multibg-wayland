// https://niri-wm.github.io/niri/IPC.html#programmatic-access

use super::*;

pub struct Niri {
    niri_socket: Box<Path>,
    subscribe_task: Option<JoinHandle<()>>,
}

impl Niri {
    pub fn new() -> Self {
        let niri_socket = env::var_os("NIRI_SOCKET").unwrap();
        Self {
            niri_socket: PathBuf::from(niri_socket).into_boxed_path(),
            subscribe_task: None,
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

    fn visible_workspace(&mut self, output: &str) -> Option<WorkspaceVisible> {
        let mut conn = self.connect();
        let workspaces = conn.workspaces();
        workspaces.into_iter()
            .find(|w| w.is_active && w.output.as_ref().unwrap() == output)
            .map(Workspace::into_visible_workspace)
    }

    fn visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        let mut conn = self.connect();
        let workspaces = conn.workspaces();
        workspaces.into_iter()
            .filter(|w| w.is_active)
            .map(Workspace::into_visible_workspace)
            .collect()
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

    fn subscribe(&mut self, event_sender: EventSender) {
        assert!(self.subscribe_task.is_none());
        let join_handle = Subscriber::new(self, event_sender).spawn();
        self.subscribe_task = Some(join_handle);
    }
}

const REQUEST_OUTPUTS: &[u8] = b"\"Outputs\"\n";
const REQUEST_WORKSPACES: &[u8] = b"\"Workspaces\"\n";
const REQUEST_EVENT_STREAM: &[u8] = b"\"EventStream\"\n";

struct Connection {
    socket: OwnedFd,
    read_buf: ReadBuf,
}

impl Connection {
    fn connect(niri_socket: &Path) -> Self {
        let socket = connect_unix(niri_socket).unwrap();
        Self {
            socket,
            read_buf: ReadBuf::new(),
        }
    }

    fn outputs(&mut self) -> HashMap<Cow<'_, str>, Output<'_>> {
        self.send(REQUEST_OUTPUTS);
        if let Response::Outputs(outputs) = self.recv() {
            outputs
        } else {
            panic!()
        }
    }

    fn workspaces(&mut self) -> Vec<Workspace<'_>> {
        self.send(REQUEST_WORKSPACES);
        if let Response::Workspaces(workspaces) = self.recv() {
            workspaces
        } else {
            panic!()
        }
    }

    fn event_stream(&mut self) {
        self.send(REQUEST_EVENT_STREAM);
        let Response::Handled = self.recv() else {
            panic!()
        };
    }

    fn send(&mut self, mut request: &[u8]) {
        while !request.is_empty() {
            match send(&self.socket, request, SendFlags::NOSIGNAL) {
                Ok(0) => panic!(),
                Ok(count) => request = &request[count..],
                Err(Errno::INTR) => (),
                Err(Errno::PIPE) => panic!("niri IPC socket disconnected"),
                Err(e) => panic!("Failed to send to niri IPC socket: {e}"),
            }
        }
    }

    fn recv(&mut self) -> Response<'_> {
        let bytes = self.recv_bytes();
        let reply: Result<Response, String> =
            serde_json::from_slice(bytes).unwrap();
        reply.unwrap()
    }

    fn recv_bytes(&mut self) -> &[u8] {
        loop {
            if let Some(bytes) = self.take_response() {
                return bytes
            }
            match read(&self.socket, self.read_buf.spare_capacity()) {
                Ok(0) => panic!("niri IPC socket disconnected"),
                Ok(_) => (),
                Err(Errno::INTR) => (),
                Err(e) => panic!("Failed to read niri IPC socket: {e}"),
            }
        }
    }

    fn take_response(&mut self) -> Option<&[u8]> {
        let unparsed = &self.read_buf.buf[self.read_buf.parsed_len..];
        let idx = memchr(unparsed, b'\n')?;
        let response = &unparsed[..idx];
        self.read_buf.parsed_len += idx + 1;
        Some(response)
    }
}

struct Subscriber {
    conn: Connection,
    event_sender: EventSender,
    workspaces: Vec<WorkspaceOwn>,
}

impl Subscriber {
    fn new(niri: &Niri, event_sender: EventSender) -> Self {
        let mut conn = niri.connect();
        conn.event_stream();
        let mut conn2 = niri.connect();
        let workspaces = conn2.workspaces().into_iter()
            .filter_map(WorkspaceOwn::from_workspace)
            .collect();
        Self { conn, event_sender, workspaces }
    }

    fn spawn(mut self) -> JoinHandle<()> {
        thread::Builder::new()
            .name(String::from("niri-connection"))
            .spawn(move || loop {
                self.cycle();
            }).unwrap()
    }

    fn cycle(&mut self) {
        let bytes = self.conn.recv_bytes();
        let event: Option<Event> = serde_json::from_slice(bytes).unwrap();
        match event.unwrap() {
            Event::WorkspacesChanged { workspaces } => {
                self.workspaces.clear();
                self.workspaces.extend(
                    workspaces.into_iter()
                        .filter_map(WorkspaceOwn::from_workspace)
                );
            },
            Event::WorkspaceActivated { id } => {
                self.event_sender.send(
                    self.workspaces.iter().find(|w| w.id == id).unwrap()
                        .to_visible_workspace()
                );
            },
            Event::Other(_) => (),
        }
    }
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

impl Workspace<'_> {
    fn into_visible_workspace(self) -> WorkspaceVisible {
        WorkspaceVisible {
            output: self.output.unwrap().into(),
            workspace_name: self.name
                .unwrap_or_else(|| format!("{}", self.idx).into())
                .into_owned(),
            workspace_number: self.idx.into(),
        }
    }
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

    fn to_visible_workspace(&self) -> WorkspaceVisible {
        WorkspaceVisible {
            output: String::from(self.output.as_ref()),
            workspace_name: String::from(self.name.as_ref()),
            workspace_number: self.idx.into(),
        }
    }
}
