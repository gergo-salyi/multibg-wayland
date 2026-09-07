use super::*;

const MAGIC: &[u8; 6] = b"i3-ipc";
const GET_WORKSPACES: u32 = 1;
const SUBSCRIBE: u32 = 2;
const GET_OUTPUTS: u32 = 3;
const EVENT_WORKSPACE: u32 = 0x80000000;

pub struct Sway {
    swaysock: Box<Path>,
    subscribe_task: Option<JoinHandle<()>>,
}

impl Sway {
    pub fn new() -> Self {
        let swaysock = env::var_os("SWAYSOCK").unwrap();
        Self {
            swaysock: PathBuf::from(swaysock).into_boxed_path(),
            subscribe_task: None,
        }
    }

    fn connect(&self) -> Connection {
        Connection::connect(&self.swaysock)
    }
}

impl CompositorCtx for Sway {
    fn list_outputs(&mut self) {
        let mut conn = self.connect();
        let outputs = conn.outputs();
        let mut stdout_lock = stdout().lock();
        outputs.into_iter().for_each(|output| {
            print_output(
                &mut stdout_lock,
                &output.name,
                &output.make,
                &output.model,
                &output.serial,
            ).unwrap();
        });
    }

    fn visible_workspace(&mut self, output: &str) -> Option<WorkspaceVisible> {
        self.connect().visible_workspace(output)
    }

    fn visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        self.connect().visible_workspaces()
    }

    fn make_model_serial(&mut self, output: &str) -> Option<Box<str>> {
        let mut conn = self.connect();
        let output = conn.outputs().into_iter()
            .find(|o| o.name == output).unwrap();
        make_model_serial(
            &output.make,
            &output.model,
            &output.serial,
        )
    }

    fn subscribe(&mut self, event_sender: EventSender) {
        assert!(self.subscribe_task.is_none());
        let join_handle = self.connect().spawn_subscribe(event_sender);
        self.subscribe_task = Some(join_handle);
    }
}

struct Connection {
    socket: OwnedFd,
    in_buf: Vec<u8>,
    parsed_len: usize,
    out_buf: Vec<u8>,
}

impl Connection {
    fn connect(swaysock: &Path) -> Self {
        let socket = connect_unix(swaysock).unwrap();
        Self {
            socket,
            in_buf: Vec::with_capacity(14),
            parsed_len: 0,
            out_buf: Vec::new(),
        }
    }

    fn send(&mut self, message_type: u32, payload: &[u8]) {
        let payload_len = payload.len();
        let encoded_payload_len = i32::try_from(payload_len).unwrap()
            .to_ne_bytes();
        let message_len = 14 + payload_len;
        if message_len > self.out_buf.capacity() {
            self.out_buf.reserve_exact(message_len - self.out_buf.capacity());
        }
        self.out_buf.clear();
        self.out_buf.extend_from_slice(MAGIC);
        self.out_buf.extend_from_slice(&encoded_payload_len);
        self.out_buf.extend_from_slice(&message_type.to_ne_bytes());
        self.out_buf.extend_from_slice(payload);
        let mut message = self.out_buf.as_slice();
        while !message.is_empty() {
            match send(&self.socket, message, SendFlags::NOSIGNAL) {
                Ok(0) => panic!(),
                Ok(count) => message = &message[count..],
                Err(Errno::INTR) => (),
                Err(Errno::PIPE) => panic!("Sway IPC socket disconnected"),
                Err(e) => panic!("Failed to send to Sway IPC socket: {e}"),
            }
        }
    }

    fn recv(&mut self) -> (u32, &[u8]) {
        loop {
            let unparsed_len = self.in_buf.len().strict_sub(self.parsed_len);
            let mut reply_len = 0;
            if unparsed_len >= 14 {
                let unparsed = &self.in_buf[self.parsed_len..];
                assert_eq!(&unparsed[0..6], MAGIC);
                let payload_len: usize = u32::from_ne_bytes(
                    unparsed[6..10].try_into().unwrap()
                ).try_into().unwrap();
                let payload_type = u32::from_ne_bytes(
                    unparsed[10..14].try_into().unwrap()
                );
                reply_len = 14 + payload_len;
                if unparsed_len >= reply_len {
                    let payload = &unparsed[14..reply_len];
                    self.parsed_len += reply_len;
                    return (payload_type, payload)
                }
            }
            if self.parsed_len != 0 {
                if unparsed_len == 0 {
                    self.in_buf.clear();
                } else {
                    self.in_buf.copy_within(self.parsed_len.., 0);
                    self.in_buf.truncate(unparsed_len);
                }
                self.parsed_len = 0;
            } else if self.in_buf.len() == self.in_buf.capacity() {
                self.in_buf.reserve_exact(
                    reply_len.strict_sub(self.in_buf.capacity())
                );
            }
            match read(&self.socket, spare_capacity(&mut self.in_buf)) {
                Ok(0) => panic!("Sway IPC socket disconnected"),
                Ok(_) => (),
                Err(Errno::INTR) => (),
                Err(e) => panic!("Failed to read Sway IPC socket: {e}"),
            }
        }
    }

    fn outputs(&mut self) -> Vec<Output<'_>> {
        self.send(GET_OUTPUTS, b"");
        let (reply_type, payload) = self.recv();
        assert_eq!(reply_type, GET_OUTPUTS);
        serde_json::from_slice(payload).unwrap()
    }

    fn visible_workspace(&mut self, output: &str) -> Option<WorkspaceVisible> {
        let workspaces = self.workspaces();
        workspaces.into_iter()
            .find(|w| w.visible && w.output == output)
            .map(|workspace| WorkspaceVisible {
                output: workspace.output.into_owned(),
                workspace_name: workspace.name.into_owned(),
                workspace_number: workspace.num,
            })
    }

    fn visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        let workspaces = self.workspaces();
        workspaces.into_iter()
            .filter(|w| w.visible)
            .map(|workspace| WorkspaceVisible {
                output: workspace.output.into_owned(),
                workspace_name: workspace.name.into_owned(),
                workspace_number: workspace.num,
            })
            .collect()
    }

    fn workspaces(&mut self) -> Vec<Workspace<'_>> {
        self.send(GET_WORKSPACES, b"");
        let (reply_type, payload) = self.recv();
        assert_eq!(reply_type, GET_WORKSPACES);
        serde_json::from_slice(payload).unwrap()
    }

    fn spawn_subscribe(
        mut self,
        event_sender: EventSender,
    ) -> JoinHandle<()> {
        self.send(SUBSCRIBE, b"[\"workspace\"]");
        let (reply_type, payload) = self.recv();
        assert_eq!(reply_type, SUBSCRIBE);
        let subscribe: Subscribe = serde_json::from_slice(payload)
            .unwrap();
        assert!(subscribe.success);
        thread::Builder::new()
            .name(String::from("sway-connection"))
            .spawn(move || loop {
                self.subscribe_loop_round(&event_sender);
            }).unwrap()
    }

    fn subscribe_loop_round(&mut self, event_sender: &EventSender) {
        let (reply_type, payload) = self.recv();
        if reply_type != EVENT_WORKSPACE {
            return
        }
        let workspace_event: WorkspaceEvent =
            serde_json::from_slice(payload).unwrap();
        if workspace_event.change != "focus" {
            return
        }
        let current_workspace = workspace_event.current.unwrap();
        event_sender.send(WorkspaceVisible {
            output: current_workspace.output.unwrap().into_owned(),
            workspace_name: current_workspace.name.unwrap().into_owned(),
            workspace_number: current_workspace.num.unwrap(),
        });
    }
}

#[derive(Deserialize)]
struct Workspace<'a> {
    num: i32,
    #[serde(borrow)]
    name: Cow<'a, str>,
    visible: bool,
    #[serde(borrow)]
    output: Cow<'a, str>,
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

#[derive(Deserialize)]
struct Subscribe {
    success: bool,
}

#[derive(Deserialize)]
struct WorkspaceEvent<'a> {
    #[serde(borrow)]
    change: Cow<'a, str>,
    #[serde(borrow)]
    current: Option<Node<'a>>,
}

#[derive(Deserialize)]
struct Node<'a> {
    num: Option<i32>,
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow)]
    output: Option<Cow<'a, str>>,
}
