use super::*;

const MAGIC: &[u8; 6] = b"i3-ipc";
const GET_WORKSPACES: u32 = 1;
const SUBSCRIBE: u32 = 2;
const GET_OUTPUTS: u32 = 3;
const EVENT_WORKSPACE: u32 = 0x80000000;

pub struct Sway {
    swaysock: Box<Path>,
    subscriber: Option<Connection>,
}

impl Sway {
    pub fn new() -> Self {
        let swaysock = env::var_os("SWAYSOCK").unwrap();
        Self {
            swaysock: PathBuf::from(swaysock).into_boxed_path(),
            subscriber: None,
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

    fn set_wallpaper(&mut self, bg_layer: &mut BgLayer) {
        let mut conn = self.connect();
        let workspaces = conn.workspaces();
        if let Some(workspace) = workspaces.into_iter()
            .find(|w| w.visible && *w.output == *bg_layer.output_name)
        {
            bg_layer.set_wallpaper(&workspace.name, workspace.num);
        }
    }

    fn set_all_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        let mut conn = self.connect();
        let workspaces = conn.workspaces();
        workspaces.into_iter().filter(|w| w.visible).for_each(|workspace|
            set_wallpaper(
                bg_layers,
                &workspace.output,
                &workspace.name,
                workspace.num,
            )
        );
    }

    fn make_model_serial(&mut self, output: &str) -> Option<Box<str>> {
        let mut conn = self.connect();
        let output = conn.outputs().into_iter()
            .find(|o| o.name == output).unwrap();
        make_model_serial(&output.make, &output.model, &output.serial)
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
    out_buf: Vec<u8>,
}

impl Connection {
    fn connect(swaysock: &Path) -> Self {
        Self {
            buf_sock: BufSock::connect(swaysock).unwrap(),
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
        let buf = &mut self.out_buf;
        buf.clear();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&encoded_payload_len);
        buf.extend_from_slice(&message_type.to_ne_bytes());
        buf.extend_from_slice(payload);
        self.buf_sock.send_all(buf.as_slice()).unwrap();
    }

    fn recv_next_reply_blocking(&mut self) -> Reply<'_> {
        loop {
            match next_reply(&mut self.buf_sock.read_buf) {
                Some(reply) => return reply,
                None => self.buf_sock.recv_blocking().unwrap(),
            }
        }
    }

    fn outputs(&mut self) -> Vec<Output<'_>> {
        self.send(GET_OUTPUTS, b"");
        let reply = self.recv_next_reply_blocking();
        assert_eq!(reply.typ, GET_OUTPUTS);
        serde_json::from_slice(reply.payload).unwrap()
    }

    fn workspaces(&mut self) -> Vec<Workspace<'_>> {
        self.send(GET_WORKSPACES, b"");
        let reply = self.recv_next_reply_blocking();
        assert_eq!(reply.typ, GET_WORKSPACES);
        serde_json::from_slice(reply.payload).unwrap()
    }

    fn subscribe(&mut self) {
        self.send(SUBSCRIBE, b"[\"workspace\"]");
        let reply = self.recv_next_reply_blocking();
        assert_eq!(reply.typ, SUBSCRIBE);
        let subscribe: Subscribe = serde_json::from_slice(reply.payload)
            .unwrap();
        assert!(subscribe.success);
        while next_reply(&mut self.buf_sock.read_buf).is_some() {}
    }

    fn read_set_wallpapers(&mut self, bg_layers: &mut [BgLayer]) {
        self.buf_sock.recv_nonblock().unwrap();
        while let Some(reply) = next_reply(&mut self.buf_sock.read_buf) {
            if reply.typ != EVENT_WORKSPACE {
                continue
            }
            let workspace_event: WorkspaceEvent =
                serde_json::from_slice(reply.payload).unwrap();
            if workspace_event.change != "focus" {
                continue
            }
            let current_workspace = workspace_event.current.unwrap();
            set_wallpaper(
                bg_layers,
                current_workspace.output.unwrap().as_ref(),
                current_workspace.name.unwrap().as_ref(),
                current_workspace.num.unwrap(),
            )
        }
    }
}

struct Reply<'a> {
    typ: u32,
    payload: &'a [u8],
}

fn next_reply(read_buf: &mut ReadBuf) -> Option<Reply<'_>> {
    let unparsed = &read_buf.buf[read_buf.parsed_len..];
    if let Some(maybe_payload) = &unparsed.get(14..) {
        assert_eq!(&unparsed[0..6], MAGIC);
        let payload_len: usize = u32::from_ne_bytes(
            unparsed[6..10].try_into().unwrap()
        ).try_into().unwrap();
        let typ = u32::from_ne_bytes(
            unparsed[10..14].try_into().unwrap()
        );
        if let Some(payload) = &maybe_payload.get(..payload_len) {
            read_buf.parsed_len += 14 + payload_len;
            return Some(Reply { typ, payload })
        }
    }
    None
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
