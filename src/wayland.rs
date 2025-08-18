use std::{
    cell::RefCell,
    os::fd::AsFd,
    path::PathBuf,
    rc::{Rc, Weak},
};

use anyhow::{bail, Context};
use log::{debug, error, warn};
use rustix::fs::{Dev, major, minor};
use smithay_client_toolkit::{
    delegate_compositor, delegate_dmabuf, delegate_layer, delegate_output,
    delegate_registry, delegate_shm,
    compositor::{CompositorHandler, Region},
    dmabuf::{DmabufFeedback, DmabufHandler, DmabufState},
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer,
            LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        raw::RawPool,
    },
};
use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_buffer::WlBuffer,
        wl_output::{self, Transform, WlOutput},
        wl_shm,
        wl_surface::WlSurface,
    },
};
use smithay_client_toolkit::reexports::protocols::wp::{
    linux_dmabuf::zv1::client::{
        zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    },
    viewporter::client::{
        wp_viewport::WpViewport,
        wp_viewporter::WpViewporter
    }
};

use crate::{
    flush_blocking, State,
    gpu::{
        DRM_FORMAT_XRGB8888, fmt_modifier,
        GpuMemory, GpuUploader, GpuWallpaper,
    },
    image::{load_wallpaper, output_wallpaper_files, WallpaperFile},
};

const MAX_FDS_OUT: usize = 28;

pub struct BackgroundLayer {
    pub output_name: String,
    width: i32,
    height: i32,
    layer: LayerSurface,
    configured: bool,
    workspace_backgrounds: Vec<WorkspaceBackground>,
    current_wallpaper: Option<Rc<RefCell<Wallpaper>>>,
    queued_wallpaper: Option<Weak<RefCell<Wallpaper>>>,
    transform: Transform,
    viewport: Option<WpViewport>,
    dmabuf_feedback: Option<ZwpLinuxDmabufFeedbackV1>,
}

impl Drop for BackgroundLayer {
    fn drop(&mut self) {
        if let Some(dmabuf_feedback) = &self.dmabuf_feedback {
            dmabuf_feedback.destroy();
        }
        if let Some(viewport) = &self.viewport {
            viewport.destroy();
        }
    }
}

impl BackgroundLayer {
    pub fn draw_workspace_bg(&mut self, workspace_name: &str, workspace_number: i32) {
        if !self.configured {
            error!("Cannot draw wallpaper image on the not yet configured \
                layer for output: {}", self.output_name);
            return
        }

        let Some(workspace_bg) = self.workspace_backgrounds.iter()
            .find(|workspace_bg| workspace_bg.workspace_name == workspace_name)
            .or_else(|| self.workspace_backgrounds.iter()
                .find(|workspace_bg| workspace_bg.workspace_number == workspace_number)
            )
            .or_else(|| self.workspace_backgrounds.iter()
                .find(|workspace_bg| workspace_bg.workspace_name == "_default")
            )
        else {
            error!(
                "There is no wallpaper image on output {} for workspace {}, \
                    only for: {}",
                self.output_name,
                workspace_name,
                self.workspace_backgrounds.iter()
                    .map(|workspace_bg| workspace_bg.workspace_name.as_str())
                    .collect::<Vec<_>>().join(", ")
            );
            return
        };
        let wallpaper = &workspace_bg.wallpaper;

        if let Some(current) = &self.current_wallpaper {
            if Rc::ptr_eq(current, wallpaper) {
                debug!("Skipping draw on output {} for workspace {} \
                    because its wallpaper is already set",
                    self.output_name, workspace_name);
                return
            }
        }

        let wallpaper_borrow = wallpaper.borrow();
        let Some(wl_buffer) = wallpaper_borrow.wl_buffer.as_ref() else {
            debug!("Wallpaper for output {} workspace {} is not ready yet",
                self.output_name, workspace_name);
            self.queued_wallpaper = Some(Rc::downgrade(wallpaper));
            return
        };

        // Attach and commit to new workspace background
        self.layer.attach(Some(wl_buffer), 0, 0);
        // wallpaper_borrow.active_count += 1;

        // Damage the entire surface
        self.layer.wl_surface().damage_buffer(0, 0, self.width, self.height);

        self.layer.commit();

        self.current_wallpaper = Some(Rc::clone(wallpaper));
        self.queued_wallpaper = None;

        debug!("Setting wallpaper on output {} for workspace: {}",
            self.output_name, workspace_name);
    }
}

struct WorkspaceBackground {
    workspace_name: String,
    workspace_number: i32,
    wallpaper: Rc<RefCell<Wallpaper>>,
}

struct Wallpaper {
    wl_buffer: Option<WlBuffer>,
    // active_count: usize,
    memory: Memory,
    canon_path: PathBuf,
    canon_modified: u128,
}

impl Drop for Wallpaper {
    fn drop(&mut self) {
        if let Some(wl_buffer) = &self.wl_buffer {
            // if self.active_count != 0 {
            //     debug!("Destroying a {} times active wl_buffer of \
            //         wallpaper {:?}", self.active_count, self.canon_path);
            // }
            wl_buffer.destroy();
        }
    }
}

enum Memory {
    WlShm { pool: RawPool },
    Dmabuf { gpu_memory: GpuMemory, params: Option<ZwpLinuxBufferParamsV1> },
}

impl Memory {
    fn gpu_uploader_eq(&self, gpu_uploader: Option<&GpuUploader>) -> bool {
        if let Some(gpu_uploader) = gpu_uploader {
            match self {
                Memory::WlShm { .. } => false,
                Memory::Dmabuf { gpu_memory, .. } => {
                    gpu_memory.gpu_uploader_eq(gpu_uploader)
                },
            }
        } else {
            match self {
                Memory::WlShm { .. } => true,
                Memory::Dmabuf { .. } => false,
            }
        }
    }

    fn dmabuf_params_destroy_eq(
        &mut self,
        other_params: &ZwpLinuxBufferParamsV1,
    ) -> bool {
        if let Memory::Dmabuf { params: params_option, .. } = self {
            if let Some(params) = params_option {
                if params == other_params {
                    params.destroy();
                    *params_option = None;
                    return true
                }
            }
        }
        false
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_factor: i32,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _time: u32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_feedback(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        proxy: &ZwpLinuxDmabufFeedbackV1,
        feedback: DmabufFeedback,
    ) {
        let Some(bg_layer_pos) = self.background_layers.iter()
            .position(|bg_layer|
                bg_layer.dmabuf_feedback.as_ref() == Some(proxy)
            )
        else {
            debug!("Received unexpected Linux DMA-BUF feedback");
            return
        };
        if let Err(e) = handle_dmabuf_feedback(
            self,
            conn,
            qh,
            feedback,
            bg_layer_pos
        ) {
            error!("Failed to proceed with DMA-BUF feedback, \
                falling back to shm: {e:#}");
            fallback_shm_load_wallpapers(self, conn, qh, bg_layer_pos);
        }
    }

    fn created(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        params: &ZwpLinuxBufferParamsV1,
        buffer: WlBuffer,
    ) {
        for bg_layer in self.background_layers.iter_mut() {
            for workspace_bg in bg_layer.workspace_backgrounds.iter_mut() {
                let wallpaper = &workspace_bg.wallpaper;
                let mut wallpaper_borrow = wallpaper.borrow_mut();
                if wallpaper_borrow.memory.dmabuf_params_destroy_eq(params) {
                    wallpaper_borrow.wl_buffer = Some(buffer);
                    debug!("Created Linux DMA-BUF buffer for wallpaper \
                        file {:?}", wallpaper_borrow.canon_path);
                    drop(wallpaper_borrow);
                    if let Some(queued_weak) = &bg_layer.queued_wallpaper {
                        if let Some(queued) = queued_weak.upgrade() {
                            if Rc::ptr_eq(&queued, wallpaper) {
                                let name = workspace_bg.workspace_name.clone();
                                let number = workspace_bg.workspace_number;
                                bg_layer.draw_workspace_bg(&name, number);
                            }
                        }
                    }
                    return
                }
            }
        }
        error!("Received unexpected created Linux DMA-BUF buffer");
    }

    fn failed(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        params: &ZwpLinuxBufferParamsV1,
    ) {
        error!("Failed to create a Linux DMA-BUF buffer");
        let mut failed_bg_layer_indecies = Vec::new();
        for (i, bg_layer) in self.background_layers.iter_mut().enumerate() {
            for workspace_bg in bg_layer.workspace_backgrounds.iter_mut() {
                let mut wallpaper = workspace_bg.wallpaper.borrow_mut();
                if wallpaper.memory.dmabuf_params_destroy_eq(params) {
                    error!("Falling back to shm and reloading wallpapers \
                        for output {}", bg_layer.output_name);
                    failed_bg_layer_indecies.push(i);
                    break
                }
            }
        }
        for index in failed_bg_layer_indecies {
            fallback_shm_load_wallpapers(self, conn, qh, index);
        }
    }

    fn released(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _buffer: &WlBuffer
    ) {
        // for bg in self.background_layers.iter_mut()
        //     .flat_map(|bg_layer| &mut bg_layer.workspace_backgrounds)
        // {
        //     let mut wallpaper = bg.wallpaper.borrow_mut();
        //     if wallpaper.wl_buffer.as_ref() == Some(buffer) {
        //         if let Some(new_count) = wallpaper.active_count.checked_sub(1) {
        //             debug!("Compositor released the DMA-BUF wl_buffer of {:?}",
        //                 wallpaper.canon_path);
        //             wallpaper.active_count = new_count;
        //         } else {
        //             error!("Unexpected release event for the DMA-BUF \
        //                 wl_buffer of {:?}", wallpaper.canon_path);
        //         }
        //         return
        //     }
        // }
        // warn!("Release event for already destroyed DMA-BUF wl_buffer");
    }
}

impl LayerShellHandler for State {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface
    ) {
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // The new layer is ready: request all the visible workspace from sway,
        // it will get picked up by the main event loop and be drawn from there
        let bg_layer = self.background_layers.iter_mut()
            .find(|bg_layer| &bg_layer.layer == layer).unwrap();

        if !bg_layer.configured {
            bg_layer.configured = true;
            self.compositor_connection_task
                .request_visible_workspace(&bg_layer.output_name);

            debug!("Configured layer on output: {}, new surface size {}x{}",
                bg_layer.output_name,
                configure.new_size.0, configure.new_size.1);
        } else {
            debug!("Ignoring configure for already configured layer \
                on output: {}, new surface size {}x{}",
                bg_layer.output_name,
                configure.new_size.0, configure.new_size.1);
        }
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        output: WlOutput,
    ) {
        let Some(info) = self.output_state.info(&output) else {
            error!("New output has no output info, skipping");
            return
        };

        let Some(output_name) = info.name else {
            error!("New output has no name, skipping");
            return
        };

        let Some((width, height)) = info.modes.iter()
            .find(|mode| mode.current)
            .map(|mode| mode.dimensions)
        else {
            error!("New output {} has no current mode set, skipping",
                output_name);
            return
        };

        if !width.is_positive() || !height.is_positive() {
            error!("New output {} has non-positive resolution: {} x {}, \
                skipping", output_name, width, height);
            return
        }

        let (width, height) = {
            match info.transform {
                Transform::Normal
                | Transform::_180
                | Transform::Flipped
                | Transform::Flipped180 => (width, height),
                Transform::_90
                | Transform::_270
                | Transform::Flipped90
                | Transform::Flipped270 => (height, width),
                _ => {
                    warn!("New output {} has unsupported transform",
                        output_name);
                    (width, height)
                }
            }
        };

        let integer_scale_factor = info.scale_factor;

        let Some((logical_width, logical_height)) = info.logical_size else {
            error!("New output {} has no logical_size, skipping", output_name);
            return
        };

        if !logical_width.is_positive() || !logical_height.is_positive() {
            error!("New output {} has non-positive logical size: {} x {}, \
                skipping", output_name, logical_width, logical_height);
            return
        }

        #[cfg(debug_assertions)]
        let (width, logical_width, height, logical_height) = {
            let mut ret = (width, logical_width, height, logical_height);
            if let Ok(var) = std::env::var("MULTIBG_DEBUG_OUTPUT_RES") {
                for out_res in var.split(',') {
                    let (output, res) = out_res.split_once('=').unwrap();
                    if output == output_name.as_str() {
                        let (w, h) = res.split_once('x').unwrap();
                        let w: i32 = w.parse().unwrap();
                        let h: i32 = h.parse().unwrap();
                        ret = (w, w, h, h);
                        break
                    }
                }
            }
            ret
        };

        debug!("New output, name: {}, resolution: {}x{}, integer scale \
            factor: {}, logical size: {}x{}, transform: {:?}",
            output_name, width, height, integer_scale_factor,
            logical_width, logical_height, info.transform);

        let layer = self.layer_shell.create_layer_surface(
            qh,
            self.compositor_state.create_surface(qh),
            Layer::Background,
            layer_surface_name(&output_name),
            Some(&output)
        );

        layer.set_anchor(
            Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT
        );
        layer.set_exclusive_zone(-1); // Don't let the status bar push it around
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);

        let surface = layer.wl_surface();

        // Disable receiving pointer, touch, and tablet events
        // by setting an empty input region.
        // This prevents disappearing or hidden cursor when a normal window
        // closes below the pointer leaving it above our surface
        match Region::new(&self.compositor_state) {
            Ok(region) => surface.set_input_region(Some(region.wl_region())),
            Err(error) => error!(
                "Failed to create empty input region, on new output {}: {}",
                output_name, error
            )
        };

        let mut viewport = None;

        if width == logical_width || height == logical_height {
            debug!("Output {} needs no scaling", output_name);
        } else if width == logical_width * integer_scale_factor
            && height == logical_height * integer_scale_factor
        {
            debug!("Output {} needs integer scaling", output_name);
            surface.set_buffer_scale(integer_scale_factor);
        } else {
            debug!("Output {} needs fractional scaling", output_name);
            let new_viewport = self.viewporter.get_viewport(surface, qh, ());
            new_viewport.set_destination(logical_width, logical_height);
            viewport = Some(new_viewport);
        }

        layer.commit();

        let mut dmabuf_feedback = None;
        let mut gpu_uploader = None;
        if let Some(gpu) = self.gpu.as_mut() {
            if self.dmabuf_state.version().unwrap() >= 4 {
                match self.dmabuf_state.get_surface_feedback(surface, qh) {
                    Ok(feedback) => {
                        debug!("Requesting Linux DMA-BUF surface feedback \
                            for output {}", output_name);
                        dmabuf_feedback = Some(feedback);
                    },
                    Err(e) => {
                        error!("Failed to request Linux DMA-BUF surface \
                            feedback for the surface on output {}: {}",
                            output_name, e);
                    },
                }
            } else {
                let drm_format_modifiers = self.dmabuf_state.modifiers().iter()
                    .filter(|dmabuf_format|
                        dmabuf_format.format == DRM_FORMAT_XRGB8888
                    )
                    .map(|dmabuf_format| dmabuf_format.modifier)
                    .collect::<Vec<_>>();
                match gpu.uploader(
                    None,
                    width as u32,
                    height as u32,
                    drm_format_modifiers,
                ) {
                    Ok(uploader) => gpu_uploader = Some(uploader),
                    Err(e) => error!("Failed to obtain GPU uploader: {e:#}"),
                };
            }
        }
        let is_dmabuf_feedback = dmabuf_feedback.is_some();
        let bg_layer_index = self.background_layers.len();
        self.background_layers.push(BackgroundLayer {
            output_name,
            width,
            height,
            layer,
            configured: false,
            workspace_backgrounds: Vec::new(),
            current_wallpaper: None,
            queued_wallpaper: None,
            transform: info.transform,
            viewport,
            dmabuf_feedback,
        });
        if !is_dmabuf_feedback {
            load_wallpapers(self, conn, qh, bg_layer_index, gpu_uploader);
        }
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: WlOutput,
    ) {
        let Some(info) = self.output_state.info(&output) else {
            error!("Updated output has no output info, skipping");
            return
        };

        let Some(output_name) = info.name else {
            error!("Updated output has no name, skipping");
            return
        };

        let Some((width, height)) = info.modes.iter()
            .find(|mode| mode.current)
            .map(|mode| mode.dimensions)
        else {
            error!("Updated output {} has no current mode set, skipping",
                output_name);
            return
        };

        if !width.is_positive() || !height.is_positive() {
            error!("Updated output {} has non-positive resolution: {} x {}, \
                skipping", output_name, width, height);
            return
        }

        let (width, height) = {
            match info.transform {
                Transform::Normal
                | Transform::_180
                | Transform::Flipped
                | Transform::Flipped180 => (width, height),
                Transform::_90
                | Transform::_270
                | Transform::Flipped90
                | Transform::Flipped270 => (height, width),
                _ => {
                    warn!("Updated output {} has unsupported transform",
                        output_name);
                    (width, height)
                }
            }
        };

        let integer_scale_factor = info.scale_factor;

        let Some((logical_width, logical_height)) = info.logical_size else {
            error!("Updated output {} has no logical_size, skipping",
                output_name);
            return
        };

        if !logical_width.is_positive() || !logical_height.is_positive() {
            error!("Updated output {} has non-positive logical size: {} x {}, \
                skipping", output_name, logical_width, logical_height);
            return
        }

        debug!("Updated output, name: {}, resolution: {}x{}, integer scale \
            factor: {}, logical size: {}x{}, transform: {:?}",
            output_name, width, height, integer_scale_factor,
            logical_width, logical_height, info.transform);

        let Some(bg_layer) = self.background_layers.iter_mut()
            .find(|bg_layers| bg_layers.output_name == output_name)
        else {
            error!("Updated output {} has no background layer, skipping",
                output_name);
            return
        };

        if bg_layer.width != width || bg_layer.height != height {
            warn!("Handling of output mode or transform changes are not yet \
                implemented. Restart this application or expect broken \
                wallpapers or low quality due to scaling");
        }

        let layer = &bg_layer.layer;
        let surface = layer.wl_surface();

        if width == logical_width || height == logical_height {
            debug!("Output {} needs no scaling", output_name);
            surface.set_buffer_scale(1);
            if let Some(old_viewport) = bg_layer.viewport.take() {
                old_viewport.destroy();
            };
        } else if width == logical_width * integer_scale_factor
            && height == logical_height * integer_scale_factor
        {
            debug!("Output {} needs integer scaling", output_name);
            surface.set_buffer_scale(integer_scale_factor);
            if let Some(old_viewport) = bg_layer.viewport.take() {
                old_viewport.destroy();
            };
        } else {
            debug!("Output {} needs fractional scaling", output_name);
            surface.set_buffer_scale(1);
            bg_layer.viewport
                .get_or_insert_with(||
                    self.viewporter.get_viewport(surface, qh, ())
                )
                .set_destination(logical_width, logical_height);
        }
        // Hyprland only applies viewport change on the next redraw
        if let Some(wallpaper) = &bg_layer.current_wallpaper {
            if let Some(wl_buffer) = &wallpaper.borrow().wl_buffer {
                layer.attach(Some(wl_buffer), 0, 0);
                layer.wl_surface().damage_buffer(0, 0, width, height);
            }
        }
        layer.commit();
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: WlOutput,
    ) {
        let Some(info) = self.output_state.info(&output) else {
            error!("Destroyed output has no output info, skipping");
            return
        };

        let Some(output_name) = info.name else {
            error!("Destroyed output has no name, skipping");
            return
        };

        debug!("Output destroyed: {}", output_name);

        if let Some(bg_layer_index) = self.background_layers.iter()
            .position(|bg_layers| bg_layers.output_name == output_name)
        {
            let removed_bg_layer = self.background_layers
                .swap_remove(bg_layer_index);

            // Workspaces on the destroyed output may have been moved anywhere
            // so reset the wallpaper on all the visible workspaces
            self.compositor_connection_task.request_visible_workspaces();

            debug!(
                "Dropping {} wallpapers on destroyed output for workspaces: {}",
                removed_bg_layer.workspace_backgrounds.len(),
                removed_bg_layer.workspace_backgrounds.iter()
                    .map(|workspace_bg| workspace_bg.workspace_name.as_str())
                    .collect::<Vec<_>>().join(", ")
            );

            drop(removed_bg_layer);
        } else {
            error!(
                "Ignoring destroyed output with unknown name '{}', \
                    known outputs were: {}",
                output_name,
                self.background_layers.iter()
                    .map(|bg_layer| bg_layer.output_name.as_str())
                    .collect::<Vec<_>>().join(", ")
            );
        }

        print_memory_stats(&self.background_layers);
    }
}

impl ProvidesRegistryState for State {
    registry_handlers![OutputState];

    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(State);
delegate_dmabuf!(State);
delegate_layer!(State);
delegate_output!(State);
delegate_registry!(State);
delegate_shm!(State);

impl Dispatch<WpViewporter, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!("wp_viewporter has no events");
    }
}

impl Dispatch<WpViewport, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!("wp_viewport has no events");
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        _event: <WlBuffer as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        // for bg in state.background_layers.iter_mut()
        //     .flat_map(|bg_layer| &mut bg_layer.workspace_backgrounds)
        // {
        //     let mut wallpaper = bg.wallpaper.borrow_mut();
        //     if wallpaper.wl_buffer.as_ref() == Some(proxy) {
        //         if let Some(new_count) = wallpaper.active_count.checked_sub(1) {
        //             debug!("Compositor released the wl_shm wl_buffer of {:?}",
        //                 wallpaper.canon_path);
        //             wallpaper.active_count = new_count;
        //         } else {
        //             error!("Unexpected release event for the wl_shm \
        //                 wl_buffer of {:?}", wallpaper.canon_path);
        //         }
        //         return
        //     }
        // }
        // warn!("Release event for already destroyed wl_shm wl_buffer");
    }
}

fn layer_surface_name(output_name: &str) -> Option<String> {
    Some([env!("CARGO_PKG_NAME"), "_wallpaper_", output_name].concat())
}

fn find_equal_wallpaper(
    background_layers: &[BackgroundLayer],
    width: i32,
    height: i32,
    transform: Transform,
    wallpaper_file: &WallpaperFile,
    gpu_uploader: Option<&GpuUploader>,
) -> Option<Rc<RefCell<Wallpaper>>> {
    for bg_layer in background_layers {
        if bg_layer.width == width
            && bg_layer.height == height
            && bg_layer.transform == transform
        {
            for bg in &bg_layer.workspace_backgrounds {
                let wallpaper = bg.wallpaper.borrow();
                if wallpaper.canon_modified == wallpaper_file.canon_modified
                    && wallpaper.canon_path == wallpaper_file.canon_path
                    && wallpaper.memory.gpu_uploader_eq(gpu_uploader)
                {
                    debug!("Reusing the wallpaper of output {} workspace {}",
                        bg_layer.output_name, bg.workspace_name);
                    return Some(Rc::clone(&bg.wallpaper));
                }
            }
        }
    }
    None
}

fn find_equal_output_wallpaper(
    workspace_backgrounds: &[WorkspaceBackground],
    wallpaper_file: &WallpaperFile,
    gpu_uploader: Option<&GpuUploader>,
) -> Option<Rc<RefCell<Wallpaper>>> {
    for bg in workspace_backgrounds {
        let wallpaper = bg.wallpaper.borrow();
        if wallpaper.canon_modified == wallpaper_file.canon_modified
            && wallpaper.canon_path == wallpaper_file.canon_path
            && wallpaper.memory.gpu_uploader_eq(gpu_uploader)
        {
            debug!("Reusing the wallpaper of workspace {}",
                bg.workspace_name);
            return Some(Rc::clone(&bg.wallpaper));
        }
    }
    None
}

fn print_memory_stats(background_layers: &[BackgroundLayer]) {
    if log::log_enabled!(log::Level::Debug) {
        let mut wl_shm_count = 0.0f32;
        let mut wl_shm_size = 0.0f32;
        let mut dmabuf_count = 0.0f32;
        let mut dmabuf_size = 0.0f32;
        for bg_layer in background_layers {
            for bg in &bg_layer.workspace_backgrounds {
                let factor = 1.0 / Rc::strong_count(&bg.wallpaper) as f32;
                match &bg.wallpaper.borrow().memory {
                    Memory::WlShm { pool } => {
                        wl_shm_count += factor;
                        wl_shm_size += factor * pool.len() as f32;
                    },
                    Memory::Dmabuf { gpu_memory, .. } => {
                        dmabuf_count += factor;
                        dmabuf_size += factor * gpu_memory.size() as f32;
                    },
                }
            }
        }
        let wl_shm_count = (wl_shm_count + 0.5) as usize;
        let wl_shm_size_kb = (wl_shm_size + 0.5) as usize / 1024;
        let dmabuf_count = (dmabuf_count + 0.5) as usize;
        let dmabuf_size_kb = (dmabuf_size + 0.5) as usize / 1024;
        debug!("Memory use: {wl_shm_size_kb} KiB from {wl_shm_count} wl_shm \
            pools, {dmabuf_size_kb} KiB from {dmabuf_count} DMA-BUFs");
    }
}

fn fallback_shm_load_wallpapers(
    state: &mut State,
    conn: &Connection,
    qh: &QueueHandle<State>,
    bg_layer_index: usize,
) {
    let bg_layer = &mut state.background_layers[bg_layer_index];
    if let Some(dmabuf_feedback) = bg_layer.dmabuf_feedback.take() {
        dmabuf_feedback.destroy();
    }
    bg_layer.workspace_backgrounds.clear();
    load_wallpapers(state, conn, qh, bg_layer_index, None);
}

fn load_wallpapers(
    state: &mut State,
    connection: &Connection,
    qh: &QueueHandle<State>,
    bg_layer_index: usize,
    mut gpu_uploader: Option<GpuUploader>,
) {
    let bg_layer = &state.background_layers[bg_layer_index];
    let output_name = bg_layer.output_name.as_str();
    let width = bg_layer.width;
    let height = bg_layer.height;
    let transform = bg_layer.transform;
    let output_dir = state.wallpaper_dir.join(output_name);
    debug!("Looking for wallpapers for new output {} in {:?}",
        output_name, output_dir);
    let wallpaper_files = match output_wallpaper_files(&output_dir) {
        Ok(wallpaper_files) => wallpaper_files,
        Err(e) => {
            error!("Failed to get wallpapers for new output {output_name} \
                form {output_dir:?}: {e:#}");
            return
        }
    };
    let shm_format = state.shm_format();
    let shm_stride = match shm_format {
        wl_shm::Format::Xrgb8888 => width as usize * 4,
        wl_shm::Format::Bgr888 => {
            // Align buffer stride:
            // - once to 4, because not being aligned to 4 caused
            //   https://github.com/gergo-salyi/multibg-wayland/issues/6
            // - and to 3, because not being aligned to 3 caused
            //   https://github.com/gergo-salyi/multibg-wayland/issues/17
            // So align stride to 4 * 3 = 12
            (width as usize * 3).next_multiple_of(12)
        },
        _ => unreachable!(),
    };
    let shm_size = shm_stride * height as usize;
    let mut workspace_backgrounds = Vec::new();
    let mut resizer = fast_image_resize::Resizer::new();
    let mut reused_count = 0usize;
    let mut loaded_count = 0usize;
    let mut error_count = 0usize;
    flush_blocking(connection);
    let mut fds_need_flush = 0usize;
    for wallpaper_file in wallpaper_files {
        if log::log_enabled!(log::Level::Debug) {
            if wallpaper_file.path == wallpaper_file.canon_path {
                debug!("Wallpaper file {:?} for workspace {}",
                    wallpaper_file.path, wallpaper_file.workspace);
            } else {
                debug!("Wallpaper file {:?} -> {:?} for workspace {}",
                    wallpaper_file.path, wallpaper_file.canon_path,
                    wallpaper_file.workspace);
            }
        }
        if let Some(wallpaper) = find_equal_output_wallpaper(
            &workspace_backgrounds,
            &wallpaper_file,
            gpu_uploader.as_ref(),
        ) {
            workspace_backgrounds.push(WorkspaceBackground {
                workspace_name: wallpaper_file.workspace,
                workspace_number: wallpaper_file.workspace_number,
                wallpaper,
            });
            reused_count += 1;
            continue
        }
        if let Some(wallpaper) = find_equal_wallpaper(
            &state.background_layers,
            width,
            height,
            transform,
            &wallpaper_file,
            gpu_uploader.as_ref(),
        ) {
            workspace_backgrounds.push(WorkspaceBackground {
                workspace_name: wallpaper_file.workspace,
                workspace_number: wallpaper_file.workspace_number,
                wallpaper,
            });
            reused_count += 1;
            continue
        }
        if let Some(uploader) = gpu_uploader.as_mut() {
            if let Err(e) = load_wallpaper(
                &wallpaper_file.path,
                uploader.staging_buffer(),
                width as u32,
                height as u32,
                width as usize * 4,
                wl_shm::Format::Xrgb8888,
                state.color_transform,
                &mut resizer,
            ) {
                error!("Failed to load wallpaper: {e:#}");
                error_count += 1;
                continue
            }
            match uploader.upload() {
                Ok(gpu_wallpaper) => {
                    let fds_count = gpu_wallpaper.memory_planes_len;
                    if fds_need_flush + fds_count > MAX_FDS_OUT {
                        flush_blocking(connection);
                        fds_need_flush = 0;
                    }
                    fds_need_flush += fds_count;
                    let wallpaper = wallpaper_dmabuf(
                        &state.dmabuf_state,
                        qh,
                        gpu_wallpaper,
                        width,
                        height,
                        wallpaper_file.canon_path,
                        wallpaper_file.canon_modified,
                    );
                    workspace_backgrounds.push(WorkspaceBackground {
                        workspace_name: wallpaper_file.workspace,
                        workspace_number: wallpaper_file.workspace_number,
                        wallpaper,
                    });
                    loaded_count += 1;
                    continue
                },
                Err(e) => {
                    error!("Failed to upload wallpaper to GPU: {e:#}");
                    gpu_uploader = None;
                    // fall back to shm
                }
            }
        }
        if fds_need_flush + 1 > MAX_FDS_OUT {
            flush_blocking(connection);
            fds_need_flush = 0;
        }
        fds_need_flush += 1;
        let mut shm_pool = match RawPool::new(shm_size, &state.shm) {
            Ok(shm_pool) => shm_pool,
            Err(e) => {
                error!("Failed to create shm pool: {e}");
                error_count += 1;
                continue
            }
        };
        if let Err(e) = load_wallpaper(
            &wallpaper_file.path,
            shm_pool.mmap(),
            width as u32,
            height as u32,
            shm_stride,
            shm_format,
            state.color_transform,
            &mut resizer,
        ) {
            error!("Failed to load wallpaper: {e:#}");
            error_count += 1;
            continue
        }
        let wl_buffer = shm_pool.create_buffer(
            0,
            width,
            height,
            shm_stride.try_into().unwrap(),
            shm_format,
            (),
            qh,
        );
        workspace_backgrounds.push(WorkspaceBackground {
            workspace_name: wallpaper_file.workspace,
            workspace_number: wallpaper_file.workspace_number,
            wallpaper: Rc::new(RefCell::new(Wallpaper {
                wl_buffer: Some(wl_buffer),
                // active_count: 0,
                memory: Memory::WlShm { pool: shm_pool },
                canon_path: wallpaper_file.canon_path,
                canon_modified: wallpaper_file.canon_modified,
            })),
        });
        loaded_count += 1;
    }
    if fds_need_flush > 0 {
        flush_blocking(connection);
    }
    debug!("Wallpapers for new output: {} loaded, {} reused, {} errors",
        loaded_count, reused_count, error_count);
    debug!("Wallpapers are available for workspaces: {}",
        workspace_backgrounds.iter()
            .map(|bg| bg.workspace_name.as_str())
            .collect::<Vec<_>>().join(", "));
    state.background_layers[bg_layer_index].workspace_backgrounds =
        workspace_backgrounds;
    print_memory_stats(&state.background_layers);
}

fn handle_dmabuf_feedback(
    state: &mut State,
    conn: &Connection,
    qh: &QueueHandle<State>,
    feedback: DmabufFeedback,
    bg_layer_pos: usize,
) -> anyhow::Result<()> {
    let bg_layer = &mut state.background_layers[bg_layer_pos];
    let main_dev = feedback.main_device() as Dev;
    let format_table = feedback.format_table();
    let tranches = feedback.tranches();
    debug!("Linux DMA-BUF feedback for output {}, main device {}:{}, \
        {} format table entries, {} tranches", &bg_layer.output_name,
        major(main_dev), minor(main_dev),
        format_table.len(), tranches.len());
    if tranches.is_empty() {
        bail!("Linux DMA-BUF feedback has 0 tranches");
    }
    let mut selected = None;
    for (index, tranche) in tranches.iter().enumerate() {
        let target_dev = tranche.device as Dev;
        debug!("Tranche {index} target device {}:{}",
            major(target_dev), minor(target_dev));
        if selected.is_none() && target_dev == main_dev {
            selected = Some((index, tranche.formats.as_slice()));
        }
    }
    let Some((index, formats)) = selected else {
        bail!("No tranche has the main device as target device");
    };
    debug!("Selected tranche {}, it has {} dmabuf formats",
        index, formats.len());
    let mut drm_format_modifiers = Vec::new();
    for index in formats {
        let Some(dmabuf_format) = format_table.get(*index as usize) else {
            error!("Format index {index} is out of bounds");
            continue
        };
        if dmabuf_format.format == DRM_FORMAT_XRGB8888 {
            drm_format_modifiers.push(dmabuf_format.modifier);
        }
    }
    #[cfg(debug_assertions)]
    if std::env::var("MULTIBG_DEBUG_GPU_FORMAT_LINEAR").is_ok() {
        drm_format_modifiers = vec![crate::gpu::DRM_FORMAT_MOD_LINEAR];
    }
    if drm_format_modifiers.is_empty() {
        bail!("Selected tranche has no modifiers for DRM_FORMAT_XRGB8888");
    }
    debug!("Modifiers for DRM_FORMAT_XRGB8888: {}",
        drm_format_modifiers.iter()
            .map(|&modifier| fmt_modifier(modifier))
            .collect::<Vec<_>>().join(", "));
    let dmabuf_drm_dev = Some(main_dev);
    if !bg_layer.workspace_backgrounds.is_empty()
        && bg_layer.workspace_backgrounds.iter().all(|bg| {
            let memory = &bg.wallpaper.borrow().memory;
            if let Memory::Dmabuf { gpu_memory, .. } = memory {
                gpu_memory.dmabuf_feedback_eq(
                    dmabuf_drm_dev,
                    &drm_format_modifiers
                )
            } else {
                false
            }
        })
    {
        debug!("Ignoring DMA-BUF feedback with no changes");
        return Ok(())
    }
    let gpu_uploader = state.gpu.as_mut().unwrap().uploader(
        dmabuf_drm_dev,
        bg_layer.width as u32,
        bg_layer.height as u32,
        drm_format_modifiers
    ).context("Failed to create GPU uploader")?;
    if !bg_layer.workspace_backgrounds.is_empty() {
        debug!("DMA-BUF feedback changed, reloading wallpapers");
        bg_layer.workspace_backgrounds.clear();
    }
    load_wallpapers(state, conn, qh, bg_layer_pos, Some(gpu_uploader));
    Ok(())
}

fn wallpaper_dmabuf(
    dmabuf_state: &DmabufState,
    qh: &QueueHandle<State>,
    gpu_wallpaper: GpuWallpaper,
    width: i32,
    height: i32,
    canon_path: PathBuf,
    canon_modified: u128,
) -> Rc<RefCell<Wallpaper>> {
    let GpuWallpaper {
        drm_format_modifier,
        memory_planes_len,
        memory_planes,
        gpu_memory,
        fd,
    } = gpu_wallpaper;
    let dmabuf_params = dmabuf_state.create_params(qh).unwrap();
    #[allow(clippy::needless_range_loop)]
    for memory_plane_index in 0..memory_planes_len {
        dmabuf_params.add(
            fd.as_fd(),
            memory_plane_index as u32,
            memory_planes[memory_plane_index].offset as u32,
            memory_planes[memory_plane_index].stride as u32,
            drm_format_modifier,
        );
    }
    let params = dmabuf_params.create(
        width,
        height,
        DRM_FORMAT_XRGB8888,
        zwp_linux_buffer_params_v1::Flags::empty(),
    );
    Rc::new(RefCell::new(Wallpaper {
        wl_buffer: None,
        // active_count: 0,
        memory: Memory::Dmabuf { gpu_memory, params: Some(params) },
        canon_path,
        canon_modified,
    }))
}
