#![deny(unused_must_use)]
#![allow(clippy::uninlined_format_args)]

mod buffer;
mod cli;
mod compositors;
mod cpu;
mod file_io;
mod gpu;
mod guard;
mod img;
mod log;
mod memchr;
mod poll;
mod signal;
mod wayland;

use std::{
    alloc::{alloc, dealloc, handle_alloc_error, Layout, realloc},
    arch::asm,
    array,
    backtrace::Backtrace,
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    env::{self, args_os, current_dir},
    ffi::{c_char, c_int, c_void, CStr, OsStr, OsString},
    fmt::{self, Debug, Display, Formatter, Write as _},
    fs::{self, read_dir},
    hint::{assert_unchecked, cold_path},
    io::{self, Cursor, stderr, StderrLock, stdout, StdoutLock, Write},
    marker::PhantomData,
    mem::{ManuallyDrop, MaybeUninit, swap, transmute},
    ops::{Deref, DerefMut, RangeBounds},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::ffi::{OsStrExt, OsStringExt},
    },
    path::{Path, PathBuf},
    process::ExitCode,
    ptr::{self, NonNull},
    rc::{Rc, Weak},
    slice,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering::Relaxed},
        mpsc::{self, channel, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::UNIX_EPOCH,
};

use anyhow::{bail, Context};
use rustix::{
    event::{poll, PollFd, PollFlags},
    io::retry_on_intr,
};
use smithay_client_toolkit::{
    compositor::CompositorState,
    dmabuf::DmabufState,
    output::OutputState,
    registry::RegistryState,
    shell::wlr_layer::LayerShell,
    shm::Shm,
};
use smithay_client_toolkit::reexports::client::{
    Connection, EventQueue,
    backend::{ReadEventsGuard, WaylandError},
    globals::registry_queue_init,
    protocol::wl_shm,
};
use smithay_client_toolkit::reexports::protocols
    ::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::{
    buffer::Buf,
    cli::Cli,
    compositors::{Compositor, CompositorCtx, EventSender, WorkspaceVisible},
    cpu::CpuFeatures,
    gpu::{
        DRM_FORMAT_XRGB8888, fmt_modifier,
        Gpu, GpuMemory, GpuUploader, GpuWallpaper,
    },
    guard::DropGuard,
    img::{
        ColorTransform, Levels, output_wallpaper_files,
        SurfaceLayout, WallpaperFile, WallpaperLoader
    },
    log::{
        error, warn, info, debug, trace, log_error, log_warn, log,
        eprintln_fmt, LogLevel, log_enabled, set_log_level,
        quote_bytes, quote_prefixed_bytes, quote_path, quote_str,
    },
    memchr::{memchr, memmem},
    poll::{pipe_cloexec_nonblock, Poll, Waker},
    signal::SignalPipe,
    wayland::BackgroundLayer,
};

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

struct State {
    compositor_state: CompositorState,
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    viewporter: WpViewporter,
    wallpaper_dir: PathBuf,
    background_layers: Vec<BackgroundLayer>,
    compositor_ctx: Box<dyn CompositorCtx>,
    cpu_features: CpuFeatures,
    color_transform: Option<ColorTransform>,
    dmabuf_state: DmabufState,
    gpu: Option<Gpu>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn run() -> Result<(), ()> {
    let cli = Cli::parse()?;
    if cli.done() {
        return Ok(())
    }
    unsafe { set_log_level(cli.log_level) }
    info!("{PKG_NAME} {PKG_VERSION}");

    let compositor = cli.compositor
        .or_else(Compositor::from_env)
        .unwrap_or(Compositor::Sway);

    let mut compositor_ctx = compositor.context();

    if cli.list_outputs() {
        compositor_ctx.list_outputs();
        return Ok(())
    }

    let wallpaper_dir = match cli.wallpaper_dir.as_ref() {
        Some(dir) => dir.canonicalize().unwrap(),
        None => current_dir().unwrap().canonicalize().unwrap(),
    };
    let cpu_features = CpuFeatures::detect();
    let color_transform = if cli.has_levels() {
        Some(ColorTransform::from_levels(cli.levels))
    } else if cli.has_brightness_contrast() {
        let levels = Levels::from_legacy(cli.brightness, cli.contrast);
        warn_brightness_contrast(cli.brightness, cli.contrast, &levels);
        Some(ColorTransform::from_levels(levels))
    } else {
        None
    };

    // ********************************
    //     Initialize wayland client
    // ********************************

    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let layer_shell = LayerShell::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let registry_state = RegistryState::new(&globals);
    let viewporter: WpViewporter = registry_state
        .bind_one(&qh, 1..=1, ()).expect("wp_viewporter not available");
    let dmabuf_state = DmabufState::new(&globals, &qh);
    let mut gpu = None;
    if cli.gpu() {
        if let Some(version) = dmabuf_state.version() {
            if version >= 4 {
                debug!("Using Linux DMA-BUF version {version}");
            } else {
                warn!("Only legacy Linux DMA-BUF version {version} is \
                    available from the compositor where it gives no \
                    information about which GPU it uses.");
                // TODO handle this better by providing cli options
                // to choose DRM device by major:minor or /dev path
            }
            match Gpu::new() {
                Ok(val) => gpu = Some(val),
                Err(e) =>
                    error!("Failed to set up GPU, disabling GPU use: {e:#}"),
            }
        } else {
            error!("Wayland protocol Linux DMA-BUF is unavailable \
                    from the compositor, disabling GPU use");
        }
    }

    if cli.has_pixel_format() {
        warn!("Option --pixelformat is deprecated an ignored");
    }

    // Sync tools for sway ipc tasks
    let (tx, rx) = channel();
    let waker = Arc::new(Waker::new().unwrap());

    let mut state = State {
        compositor_state,
        registry_state,
        output_state: OutputState::new(&globals, &qh),
        shm,
        layer_shell,
        viewporter,
        wallpaper_dir,
        background_layers: Vec::new(),
        compositor_ctx,
        cpu_features,
        color_transform,
        dmabuf_state,
        gpu,
    };

    event_queue.roundtrip(&mut state).unwrap();

    debug!("Initial wayland roundtrip done. Starting main event loop.");

    // ********************************
    //     Main event loop
    // ********************************

    let mut poll = Poll::with_capacity(3);
    let token_wayland = poll.add_readable(&conn);
    state.compositor_ctx.subscribe(EventSender::new(tx, waker.clone()));
    let token_compositor = poll.add_readable(&waker);
    let signal_pipe = SignalPipe::new()
        .map_err(|e| error!("Failed to set up signal handling: {e}"))
        .ok();
    let token_signal = signal_pipe.as_ref().map(|pipe| poll.add_readable(pipe));

    loop {
        flush_blocking(&conn);
        let read_guard = ensure_prepare_read(&mut state, &mut event_queue);
        poll.poll().expect("Main event loop poll failed");
        if poll.ready(token_wayland) {
            handle_wayland_event(&mut state, &mut event_queue, read_guard);
        } else {
            drop(read_guard);
        }
        if poll.ready(token_compositor) {
            waker.read();
            handle_sway_event(&mut state, &rx);
        }
        if let Some(token_signal) = token_signal
            && poll.ready(token_signal)
        {
            match signal_pipe.as_ref().unwrap().read() {
                Err(e) => error!("Failed to read the signal pipe: {e}"),
                Ok(signal_flags) => {
                    if let Some(signal) = signal_flags.any_termination() {
                        info!("Received signal {signal}, exiting");
                        return Ok(());
                    } else if signal_flags.has_usr1()
                        || signal_flags.has_usr2()
                    {
                        error!("Received signal USR1 or USR2 is \
                            reserved for future functionality");
                    }
                },
            }
        }
    }
}

fn flush_blocking(connection: &Connection) {
    loop {
        let result = connection.flush();
        if result.is_ok() { return }
        if let Err(WaylandError::Io(io_error)) = &result
            && io_error.kind() == io::ErrorKind::WouldBlock
        {
            warn!("Wayland flush needs to block");
            let mut poll_fds = [PollFd::from_borrowed_fd(
                connection.as_fd(),
                PollFlags::OUT,
            )];
            retry_on_intr(|| poll(&mut poll_fds, None)).unwrap();
            continue
        }
        result.expect("Failed to flush Wayland event queue");
    }
}

fn ensure_prepare_read(
    state: &mut State,
    event_queue: &mut EventQueue<State>,
) -> ReadEventsGuard {
    loop {
        if let Some(guard) = event_queue.prepare_read() { return guard }
        event_queue.dispatch_pending(state)
            .expect("Failed to dispatch pending Wayland events");
    }
}

fn handle_wayland_event(
    state: &mut State,
    event_queue: &mut EventQueue<State>,
    read_guard: ReadEventsGuard,
) {
    match read_guard.read() {
        Ok(_) => {
            event_queue.dispatch_pending(state)
                .expect("Failed to dispatch pending Wayland events");
        },
        Err(error) => {
            if let WaylandError::Io(io_error) = &error
                && io_error.kind() == io::ErrorKind::WouldBlock
            {
                return
            }
            panic!("Failed to read Wayland events: {error}");
        }
    }
}

fn handle_sway_event(
    state: &mut State,
    rx: &Receiver<WorkspaceVisible>,
) {
    while let Ok(workspace) = rx.try_recv() {
        handle_visible_workspace(state, workspace);
    }
}

fn handle_visible_workspace(state: &mut State, workspace: WorkspaceVisible) {
    // Find the background layer that of the output where the workspace is
    if let Some(affected_bg_layer) = state.background_layers.iter_mut()
        .find(|bg_layer| *bg_layer.output_name == *workspace.output)
    {
        affected_bg_layer.draw_workspace_bg(
            &workspace.workspace_name,
            workspace.workspace_number,
        );
    } else {
        error!(
            "Workspace '{}' is on an unknown output '{}', \
            known outputs were: {}",
            workspace.workspace_name,
            workspace.output,
            state.background_layers.iter()
            .map(|bg_layer| &*bg_layer.output_name)
            .collect::<Vec<_>>().join(", ")
        );
    }
}

fn warn_brightness_contrast(
    brightness: i32,
    contrast: f32,
    levels: &Levels,
) {
    warn!("{}", fmt::from_fn(|w| {
        write!(w, "Options --brightness and --contrast are deprecated, \
            they are difficult to use correctly. Use the --level-* \
            options instead, see --help. Current")?;
        if brightness != 0 {
            write!(w, " --brightness={}", brightness)?;
        }
        if contrast != 0.0 {
            write!(w, " --contrast={}", contrast)?;
        }
        write!(w, " is equivalent to")?;
        if levels.output_min != 0.0 {
            write!(w, " --level-output-min={}", levels.output_min)?;
        }
        if levels.output_max != 1.0 {
            write!(w, " --level-output-max={}", levels.output_max)?;
        }
        if levels.input_min != 0.0 {
            write!(w, " --level-input-min={}", levels.input_min)?;
        }
        if levels.input_max != 1.0 {
            write!(w, " --level-input-max={}", levels.input_max)?;
        }
        Ok(())
    }));
}
