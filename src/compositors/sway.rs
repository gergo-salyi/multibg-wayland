use super::*;

use std::{
    borrow::Cow,
    os::fd::OwnedFd,
};

use rustix::{
    buffer::spare_capacity,
    io::{Errno, read},
    net::{
        AddressFamily, connect, send, SendFlags,
        socket_with, SocketAddrUnix, SocketFlags, SocketType,
    },
};

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

    fn connect(&self) -> SwayConnectionTask {
        SwayConnectionTask::connect(&self.swaysock)
    }
}

impl CompositorCtx for Sway {
    fn list_outputs(&mut self) {
        let outputs = self.connect().outputs();
        let mut stdout = stdout().lock();
        for output in outputs {
            writeln!(
                stdout,
                "{}: '{}'",
                output.name,
                output.make_model_serial,
            ).unwrap();
        }
    }

    fn visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        self.connect().request_visible_workspaces()
    }

    fn make_model_serial(&mut self, output: &str) -> String {
        self.connect().outputs()
            .find(|output_info| output_info.name == output).unwrap()
            .make_model_serial
    }

    fn subscribe(&mut self, event_sender: EventSender) {
        assert!(self.subscribe_task.is_none());
        let conn = self.connect();
        let join_handle = thread::Builder::new()
            .name(String::from("sway-connection"))
            .spawn(|| conn.subscribe_event_loop(event_sender)).unwrap();
        self.subscribe_task = Some(join_handle);
    }
}

pub struct SwayConnectionTask {
    socket: OwnedFd,
    in_buf: Vec<u8>,
    parsed_len: usize,
    out_buf: Vec<u8>,
}

impl SwayConnectionTask {
    fn connect(swaysock: &Path) -> Self {
        let socket = socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        ).unwrap();
        connect(
            &socket,
            &SocketAddrUnix::new(swaysock).unwrap(),
        ).unwrap();
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

    fn outputs(&mut self) -> impl Iterator<Item = OutputInfo> + use<> {
        self.send(GET_OUTPUTS, b"");
        let (reply_type, payload) = self.recv();
        assert_eq!(reply_type, GET_OUTPUTS);
        let outputs: Vec<Output> = serde_json::from_slice(payload)
            .unwrap();
        outputs.into_iter()
            .map(|output| OutputInfo {
                name: output.name.into_owned(),
                make_model_serial: make_model_serial(
                    output.make.as_ref(),
                    output.model.as_ref(),
                    output.serial.as_ref(),
                ),
            })
    }
}

impl CompositorInterface for SwayConnectionTask {
    fn request_visible_workspaces(&mut self) -> Vec<WorkspaceVisible> {
        self.send(GET_WORKSPACES, b"");
        let (reply_type, payload) = self.recv();
        assert_eq!(reply_type, GET_WORKSPACES);
        let workspaces: Vec<Workspace> = serde_json::from_slice(payload)
            .unwrap();
        workspaces.into_iter()
            .filter(|w| w.visible)
            .map(|workspace| WorkspaceVisible {
                output: workspace.output.into_owned(),
                workspace_name: workspace.name.into_owned(),
                workspace_number: workspace.num,
            })
            .collect()
    }

    fn request_outputs(&mut self) -> Vec<OutputInfo> {
        self.send(GET_OUTPUTS, b"");
        let (reply_type, payload) = self.recv();
        assert_eq!(reply_type, GET_OUTPUTS);
        let outputs: Vec<Output> = serde_json::from_slice(payload)
            .unwrap();
        outputs.into_iter()
            .map(|output| OutputInfo {
                name: output.name.into_owned(),
                make_model_serial: make_model_serial(
                    output.make.as_ref(),
                    output.model.as_ref(),
                    output.serial.as_ref(),
                ),
            })
            .collect()
    }

    fn subscribe_event_loop(mut self, event_sender: EventSender) {
        self.send(SUBSCRIBE, b"[\"workspace\"]");
        let (reply_type, payload) = self.recv();
        assert_eq!(reply_type, SUBSCRIBE);
        let subscribe: Subscribe = serde_json::from_slice(payload)
            .unwrap();
        assert!(subscribe.success);
        loop {
            let (reply_type, payload) = self.recv();
            if reply_type != EVENT_WORKSPACE {
                continue
            }
            let workspace_event: WorkspaceEvent =
                serde_json::from_slice(payload).unwrap();
            if workspace_event.change != "focus" {
                continue
            }
            let current_workspace = workspace_event.current.unwrap();
            event_sender.send(WorkspaceVisible {
                output: current_workspace.output.unwrap().into_owned(),
                workspace_name: current_workspace.name.unwrap().into_owned(),
                workspace_number: current_workspace.num.unwrap(),
            });
        }
    }
}

#[derive(Deserialize)]
struct Workspace<'a> {
    num: i32,
    name: Cow<'a, str>,
    visible: bool,
    output: Cow<'a, str>,
}

#[derive(Deserialize)]
struct Output<'a> {
    name: Cow<'a, str>,
    make: Cow<'a, str>,
    model: Cow<'a, str>,
    serial: Cow<'a, str>,
}

#[derive(Deserialize)]
struct Subscribe {
    success: bool,
}

#[derive(Deserialize)]
struct WorkspaceEvent<'a> {
    change: Cow<'a, str>,
    current: Option<Node<'a>>,
}

#[derive(Deserialize)]
struct Node<'a> {
    num: Option<i32>,
    name: Option<Cow<'a, str>>,
    output: Option<Cow<'a, str>>,
}
