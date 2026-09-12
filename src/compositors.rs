mod hyprland;
mod niri;
mod sway;

use super::*;

use rustix::{
    buffer::{SpareCapacity, spare_capacity},
    io::{Errno, read},
    net::{
        AddressFamily, connect, recv, RecvFlags, send, SendFlags,
        socket_with, SocketAddrUnix, SocketFlags, SocketType,
    },
};
use serde::{Deserialize, de::IgnoredAny};

#[derive(Clone, Copy, Debug)]
pub enum Compositor {
    Hyprland,
    Niri,
    Sway,
}

impl Compositor {
    pub fn from_env() -> Option<Compositor> {
        Compositor::from_xdg_desktop_var("XDG_SESSION_DESKTOP")
            .or_else(|| Compositor::from_xdg_desktop_var("XDG_CURRENT_DESKTOP"))
            .or_else(Compositor::from_ipc_socket_var)
    }

    fn from_xdg_desktop_var(xdg_desktop_var: &str) -> Option<Compositor> {
        if let Some(xdg_desktop) = env::var_os(xdg_desktop_var) {
            if xdg_desktop.as_bytes().starts_with(b"sway") {
                debug_selecting("Sway", xdg_desktop_var);
                Some(Compositor::Sway)
            } else if xdg_desktop.as_bytes().starts_with(b"Hyprland") {
                debug_selecting("Hyprland", xdg_desktop_var);
                Some(Compositor::Hyprland)
            } else if xdg_desktop.as_bytes().starts_with(b"niri") {
                debug_selecting("niri", xdg_desktop_var);
                Some(Compositor::Niri)
            } else {
                warn!("Unrecognized compositor from {xdg_desktop_var} \
                    environment variable: {xdg_desktop:?}");
                None
            }
        } else {
            None
        }
    }

    fn from_ipc_socket_var() -> Option<Compositor> {
        if env::var_os("SWAYSOCK").is_some() {
            debug_selecting("Sway", "SWAYSOCK");
            Some(Compositor::Sway)
        } else if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            debug_selecting("Hyprland", "HYPRLAND_INSTANCE_SIGNATURE");
            Some(Compositor::Hyprland)
        } else if env::var_os("NIRI_SOCKET").is_some() {
            debug_selecting("niri", "NIRI_SOCKET");
            Some(Compositor::Niri)
        } else {
            None
        }
    }

    pub fn context(self) -> Box<dyn CompositorCtx> {
        match self {
            Compositor::Sway => Box::new(sway::Sway::new()),
            Compositor::Hyprland => Box::new(hyprland::Hyprland::new()),
            Compositor::Niri => Box::new(niri::Niri::new()),
        }
    }
}

pub trait CompositorCtx {
    fn list_outputs(&mut self);
    fn set_wallpaper(&mut self, bg_layer: &mut BackgroundLayer);
    fn set_all_wallpapers(&mut self, bg_layers: &mut [BackgroundLayer]);
    fn make_model_serial(&mut self, output: &str) -> Option<Box<str>>;
    fn subscribe(&mut self) -> BorrowedFd<'_>;
    fn read_set_wallpapers(&mut self, bg_layers: &mut [BackgroundLayer]);
}

fn make_model_serial(
    make: &str,
    model: &str,
    serial: &str,
) -> Option<Box<str>> {
    let mut len = 0;
    for s in [make, model, serial] {
        if !s.is_empty() {
            if len != 0 {
                len += 1;
            }
            len += s.len();
        }
    }
    if len == 0 {
        return None
    }
    let mut string = String::with_capacity(len);
    for s in [make, model, serial] {
        if !s.is_empty() {
            if !string.is_empty() {
               string.push(' ');
            }
            string.push_str(s);
        }
    }
    debug_assert_eq!(string.len(), string.capacity());
    Some(string.into_boxed_str())
}

fn print_output(
    stdout_lock: &mut StdoutLock,
    name: &str,
    make: &str,
    model: &str,
    serial: &str,
) -> io::Result<()> {
    let make_model_serial = make_model_serial(make, model, serial);
    writeln!(
        stdout_lock,
        "{}: {}",
        name,
        quote_str(make_model_serial.as_deref().unwrap_or(""))
    )
}

fn debug_selecting(compositor: &str, reason: &str) {
    debug!("Selecting compositor {compositor} based on {reason}");
}

fn connect_unix(path: &Path) -> Option<OwnedFd> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    ).ok()?;
    connect(
        &socket,
        &SocketAddrUnix::new(path).ok()?,
    ).ok()?;
    Some(socket)
}

struct ReadBuf {
    buf: Vec<u8>,
    parsed_len: usize,
}

impl ReadBuf {
    fn new() -> Self {
        Self { buf: Vec::with_capacity(1000), parsed_len: 0 }
    }

    fn unparsed_len(&self) -> usize {
        self.buf.len().strict_sub(self.parsed_len)
    }

    fn spare_capacity(&mut self) -> SpareCapacity<'_, u8> {
        self.consolidate();
        self.reserve();
        spare_capacity(&mut self.buf)
    }

    fn consolidate(&mut self) {
        if self.parsed_len != 0 {
            let unparsed_len = self.unparsed_len();
            if unparsed_len == 0 {
                self.buf.clear();
            } else {
                self.buf.copy_within(self.parsed_len.., 0);
                self.buf.truncate(unparsed_len);
            }
            self.parsed_len = 0;
        }
    }

    fn reserve(&mut self) {
        let capacity = self.buf.capacity();
        if self.buf.len() == capacity {
            self.buf.reserve_exact(capacity / 2);
        }
    }
}

struct BufSock {
    socket: OwnedFd,
    read_buf: ReadBuf,
}

impl BufSock {
    fn connect(path: &Path) -> Option<Self> {
        Some(Self {
            socket: connect_unix(path)?,
            read_buf: ReadBuf::new(),
        })
    }

    fn recv_blocking(&mut self) -> Result<(), ReadError> {
        loop {
            match read(&self.socket, self.read_buf.spare_capacity()) {
                Err(Errno::INTR) => (),
                Ok(0) => break Err(ReadError::Eof),
                Ok(_) => break Ok(()),
                Err(errno) => break Err(ReadError::Errno(errno)),
            }
        }
    }

    fn recv_nonblock(&mut self) -> Result<(), ReadError> {
        match recv(
            &self.socket,
            self.read_buf.spare_capacity(),
            RecvFlags::DONTWAIT,
        ) {
            #[allow(unreachable_patterns)]
            Err(Errno::AGAIN) | Err(Errno::WOULDBLOCK) => panic!(),
            Err(Errno::INTR) => panic!(),
            Ok((0, _)) => Err(ReadError::Eof),
            Ok((_, _)) => Ok(()),
            Err(errno) => Err(ReadError::Errno(errno)),
        }
    }

    fn send_all(&self, mut buf: &[u8]) -> Result<(), Errno> {
        while !buf.is_empty() {
            match send(&self.socket, buf, SendFlags::NOSIGNAL) {
                Ok(0) => panic!(),
                Ok(count) => buf = &buf[count..],
                Err(Errno::INTR) => (),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl AsFd for BufSock {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

#[derive(Debug)]
enum ReadError {
    Errno(#[allow(dead_code)] Errno),
    Eof,
}
