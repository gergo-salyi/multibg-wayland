#![allow(unsafe_op_in_unsafe_fn)]

// https://registry.khronos.org/vulkan/specs/latest/man/html/VK_EXT_image_drm_format_modifier.html

mod device;
mod instance;
mod memory;

use std::{
    ffi::CStr,
    os::fd::OwnedFd,
    rc::{Rc, Weak},
    slice,
};

use anyhow::Context;
use ash::{
    Device, Entry, Instance,
    ext::{
        debug_report,
        debug_utils,
        image_drm_format_modifier,
    },
    khr::external_memory_fd,
    vk::{
        Buffer,
        CommandBuffer,
        CommandPool,
        DebugReportCallbackEXT,
        DebugUtilsMessengerEXT,
        DeviceMemory,
        DrmFormatModifierPropertiesEXT,
        ExtensionProperties,
        Extent2D,
        Image,
        PhysicalDevice,
        PhysicalDeviceMemoryProperties,
        Queue,
    }
};
use log::{debug, error};
use rustix::fs::Dev;

use device::device;
use instance::instance;
use memory::{upload, uploader};

pub struct Gpu {
    instance: Rc<GpuInstance>,
    devices: Vec<Weak<GpuDevice>>,
}

impl Gpu {
    pub fn new() -> anyhow::Result<Gpu> {
        let instance = Rc::new(unsafe {
            instance()
        }.context("Failed to create Vulkan instance")?);
        let devices = Vec::new();
        Ok(Gpu { instance, devices })
    }

    pub fn uploader(
        &mut self,
        dmabuf_drm_dev: Option<Dev>,
        width: u32,
        height: u32,
        drm_format_modifiers: Vec<u64>,
    ) -> anyhow::Result<GpuUploader> {
        unsafe {
            let mut selected = self.select_device(dmabuf_drm_dev);
            if selected.is_none() {
                let new_device = Rc::new(device(&self.instance, dmabuf_drm_dev)
                    .context("Failed to create new device")?);
                self.devices.push(Rc::downgrade(&new_device));
                selected = Some(new_device);
            }
            let gpu_device = selected.unwrap();
            uploader(gpu_device, width, height, drm_format_modifiers)
                .context("Failed to create GPU uploader")
        }
    }

    fn select_device(
        &mut self,
        dmabuf_drm_dev: Option<Dev>
    ) -> Option<Rc<GpuDevice>> {
        let mut ret = None;
        self.devices.retain(|weak_gpu_device| {
            if let Some(gpu_device) = weak_gpu_device.upgrade() {
                if ret.is_none()
                    && gpu_device.dmabuf_drm_dev_eq(dmabuf_drm_dev)
                {
                    ret = Some(gpu_device)
                }
                true
            } else {
                false
            }
        });
        ret
    }
}

struct GpuInstance {
    _entry: Entry,
    instance: Instance,
    debug: Debug,
}

impl Drop for GpuInstance {
    fn drop(&mut self) {
        unsafe {
            match &self.debug {
                Debug::Utils { instance, messenger } => {
                    instance.destroy_debug_utils_messenger(*messenger, None);
                },
                Debug::Report { instance, callback } => {
                    #[allow(deprecated)]
                    instance.destroy_debug_report_callback(*callback, None);
                }
                Debug::None => (),
            };
            self.instance.destroy_instance(None);
            debug!("Vulkan context has been cleaned up");
        }
    }
}

enum Debug {
    Utils {
        instance: debug_utils::Instance,
        messenger: DebugUtilsMessengerEXT,
    },
    Report {
        instance: debug_report::Instance,
        callback: DebugReportCallbackEXT,
    },
    None,
}

struct GpuDevice {
    gpu_instance: Rc<GpuInstance>,
    physdev: PhysicalDevice,
    primary_drm_dev: Option<Dev>,
    render_drm_dev: Option<Dev>,
    dmabuf_drm_dev: Option<Dev>,
    memory_props: PhysicalDeviceMemoryProperties,
    drm_format_props: Option<Vec<DrmFormatModifierPropertiesEXT>>,
    device: Device,
    external_memory_fd_device: external_memory_fd::Device,
    image_drm_format_modifier_device: Option<image_drm_format_modifier::Device>,
    queue_family_index: u32,
    queue: Queue,
    command_pool: CommandPool,
    command_buffer: CommandBuffer,
}

impl Drop for GpuDevice {
    fn drop(&mut self) {
        unsafe {
            if let Err(e) = self.device.device_wait_idle() {
                error!("Failed to wait device idle: {e}");
            };
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
        }
    }
}

impl GpuDevice {
    fn dmabuf_drm_dev_eq(&self, drm_dev: Option<Dev>) -> bool {
        if drm_dev.is_some() {
            assert!(self.dmabuf_drm_dev.is_some());
            drm_dev == self.dmabuf_drm_dev
                || drm_dev == self.render_drm_dev
                || drm_dev == self.primary_drm_dev
        } else {
            assert!(self.dmabuf_drm_dev.is_none());
            true
        }
    }
}

pub struct GpuUploader {
    gpu_device: Rc<GpuDevice>,
    buffer: Buffer,
    memory: DeviceMemory,
    ptr: *mut u8,
    len: usize,
    extent: Extent2D,
    drm_format_modifiers: Vec<u64>,
}

impl Drop for GpuUploader {
    fn drop(&mut self) {
        unsafe {
            let device = &self.gpu_device.device;
            device.unmap_memory(self.memory);
            device.free_memory(self.memory, None);
            device.destroy_buffer(self.buffer, None);
        }
    }
}

impl GpuUploader {
    pub fn staging_buffer(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    pub fn upload(&mut self) -> anyhow::Result<GpuWallpaper> {
        unsafe { upload(self) }
    }
}

pub struct GpuWallpaper {
    pub drm_format_modifier: u64,
    pub memory_planes_len: usize,
    pub memory_planes: [MemoryPlane; 4],
    pub gpu_memory: GpuMemory,
    pub fd: OwnedFd,
}

#[derive(Clone, Copy, Default)]
pub struct MemoryPlane {
    pub offset: u64,
    pub stride: u64,
}

pub struct GpuMemory {
    gpu_device: Rc<GpuDevice>,
    image: Image,
    memory: DeviceMemory,
    size: usize,
    drm_format_modifier: u64,
}

impl Drop for GpuMemory {
    fn drop(&mut self) {
        unsafe {
            self.gpu_device.device.destroy_image(self.image, None);
            self.gpu_device.device.free_memory(self.memory, None);
        }
    }
}

impl GpuMemory {
    pub fn gpu_uploader_eq(&self, gpu_uploader: &GpuUploader) -> bool {
        self.dmabuf_feedback_eq(
            gpu_uploader.gpu_device.dmabuf_drm_dev,
            gpu_uploader.drm_format_modifiers.as_slice(),
        )
    }

    pub fn dmabuf_feedback_eq(
        &self,
        dmabuf_drm_dev: Option<Dev>,
        drm_format_modifiers: &[u64]
    ) -> bool {
        self.gpu_device.dmabuf_drm_dev_eq(dmabuf_drm_dev)
            && drm_format_modifiers.contains(&self.drm_format_modifier)
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

// Fourcc codes are based on libdrm drm_fourcc.h
// https://gitlab.freedesktop.org/mesa/drm/-/blob/main/include/drm/drm_fourcc.h
// /usr/include/libdrm/drm_fourcc.h
// under license MIT
pub const fn fourcc_code(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}

pub const fn fourcc_mod_code(vendor: u64, val: u64) -> u64 {
    (vendor << 56) | (val & 0x00ff_ffff_ffff_ffff)
}

// pub const DRM_FORMAT_INVALID: u32 = 0;
pub const DRM_FORMAT_XRGB8888: u32 = fourcc_code(b'X', b'R', b'2', b'4');
// pub const DRM_FORMAT_ARGB8888: u32 = fourcc_code(b'A', b'R', b'2', b'4');

pub const DRM_FORMAT_MOD_VENDOR_NONE: u64 = 0;
// pub const DRM_FORMAT_RESERVED: u64 = (1 << 56) - 1;

// pub const DRM_FORMAT_MOD_INVALID: u64 = fourcc_mod_code(
//     DRM_FORMAT_MOD_VENDOR_NONE,
//     DRM_FORMAT_RESERVED,
// );
pub const DRM_FORMAT_MOD_LINEAR: u64 = fourcc_mod_code(
    DRM_FORMAT_MOD_VENDOR_NONE,
    0,
);

fn has_extension(extensions: &[ExtensionProperties], name: &CStr) -> bool {
    extensions.iter().any(|ext| ext.extension_name_as_c_str() == Ok(name))
}

pub fn fmt_modifier(drm_format_modifier: u64) -> String {
    format!("{drm_format_modifier:016x}")
}
