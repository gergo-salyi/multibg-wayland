mod cli;
mod compositors;
mod gpu;
mod image;
mod poll;
mod signal;
mod wayland;

use std::{
    io,
    os::fd::AsFd,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{channel, Receiver},
    },
};

use clap::Parser;
use log::{debug, error, info, warn};
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
    cli::{Cli, PixelFormat},
    compositors::{Compositor, ConnectionTask, WorkspaceVisible},
    gpu::Gpu,
    image::ColorTransform,
    poll::{Poll, Waker},
    signal::SignalPipe,
    wayland::BackgroundLayer,
};

struct State {
    compositor_state: CompositorState,
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    viewporter: WpViewporter,
    wallpaper_dir: PathBuf,
    shm_format: Option<wl_shm::Format>,
    background_layers: Vec<BackgroundLayer>,
    compositor_connection_task: ConnectionTask,
    color_transform: ColorTransform,
    dmabuf_state: DmabufState,
    gpu: Option<Gpu>,
}

impl State {
    fn shm_format(&mut self) -> wl_shm::Format {
        *self.shm_format.get_or_insert_with(|| {
            let mut format = wl_shm::Format::Xrgb8888;
            // Consume less gpu memory by using Bgr888 if available,
            // fall back to the always supported Xrgb8888 otherwise
            if self.shm.formats().contains(&wl_shm::Format::Bgr888) {
                format = wl_shm::Format::Bgr888
            }
            debug!("Using shm format: {format:?}");
            format
        })
    }
}

fn main() -> Result<(), ()> {
    run().map_err(|e| { error!("{e:#}"); })
}

fn run() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(
            #[cfg(debug_assertions)]
            "info,multibg_sway=trace",
            #[cfg(not(debug_assertions))]
            "info",
        )
    ).init();

    info!(concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION")));

    let cli = Cli::parse();
    let wallpaper_dir = Path::new(&cli.wallpaper_dir).canonicalize().unwrap();
    let brightness = cli.brightness.unwrap_or(0);
    let contrast = cli.contrast.unwrap_or(0.0);
    let color_transform = if brightness == 0 && contrast == 0.0 {
        ColorTransform::None
    } else {
        ColorTransform::Legacy { brightness, contrast }
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
    let shm_format = if cli.pixelformat == Some(PixelFormat::Baseline) {
        debug!("Using shm format: {:?}", wl_shm::Format::Xrgb8888);
        Some(wl_shm::Format::Xrgb8888)
    } else {
        None
    };

    let registry_state = RegistryState::new(&globals);

    let viewporter: WpViewporter = registry_state
        .bind_one(&qh, 1..=1, ()).expect("wp_viewporter not available");

    let dmabuf_state = DmabufState::new(&globals, &qh);
    let mut gpu = None;
    if cli.gpu {
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

    // Sync tools for sway ipc tasks
    let (tx, rx) = channel();
    let waker = Arc::new(Waker::new().unwrap());

    let compositor = cli.compositor
        .or_else(Compositor::from_env)
        .unwrap_or(Compositor::Sway);

    let mut state = State {
        compositor_state,
        registry_state,
        output_state: OutputState::new(&globals, &qh),
        shm,
        layer_shell,
        viewporter,
        wallpaper_dir,
        shm_format,
        background_layers: Vec::new(),
        compositor_connection_task: ConnectionTask::new(
            compositor, tx.clone(), Arc::clone(&waker)
        ),
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
    ConnectionTask::spawn_subscribe_event_loop(compositor, tx, waker.clone());
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
        if let Some(token_signal) = token_signal {
            if poll.ready(token_signal) {
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
}

fn flush_blocking(connection: &Connection) {
    loop {
        let result = connection.flush();
        if result.is_ok() { return }
        if let Err(WaylandError::Io(io_error)) = &result {
            if io_error.kind() == io::ErrorKind::WouldBlock {
                warn!("Wayland flush needs to block");
                let mut poll_fds = [PollFd::from_borrowed_fd(
                    connection.as_fd(),
                    PollFlags::OUT,
                )];
                retry_on_intr(|| poll(&mut poll_fds, -1)).unwrap();
                continue
            }
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
            if let WaylandError::Io(io_error) = &error {
                if io_error.kind() == io::ErrorKind::WouldBlock {
                    return
                }
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
        // Find the background layer that of the output where the workspace is
        if let Some(affected_bg_layer) = state.background_layers.iter_mut()
            .find(|bg_layer| bg_layer.output_name == workspace.output)
        {
            affected_bg_layer.draw_workspace_bg(&workspace.workspace_name);
        } else {
            error!(
                "Workspace '{}' is on an unknown output '{}', \
                    known outputs were: {}",
                workspace.workspace_name,
                workspace.output,
                state.background_layers.iter()
                    .map(|bg_layer| bg_layer.output_name.as_str())
                    .collect::<Vec<_>>().join(", ")
            );
            continue
        };
    }
}
