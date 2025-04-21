use std::{
    ffi::{CStr, c_char},
    rc::Rc,
};

use anyhow::{bail, Context};
use ash::{
    Instance,
    ext::{
        external_memory_dma_buf,
        image_drm_format_modifier,
        physical_device_drm,
        queue_family_foreign,
    },
    khr::{
        driver_properties,
        external_memory_fd,
        image_format_list,
    },
    vk::{
        self,
        api_version_variant,
        api_version_major,
        api_version_minor,
        api_version_patch,
        CommandBufferAllocateInfo,
        CommandBufferLevel,
        CommandPoolCreateFlags,
        CommandPoolCreateInfo,
        DeviceCreateInfo,
        DeviceQueueCreateInfo,
        DrmFormatModifierPropertiesEXT,
        DrmFormatModifierPropertiesListEXT,
        ExtensionProperties,
        Format,
        FormatProperties2,
        PhysicalDevice,
        PhysicalDeviceDriverProperties,
        PhysicalDeviceDrmPropertiesEXT,
        PhysicalDeviceProperties,
        PhysicalDeviceProperties2,
        PhysicalDeviceType,
        QueueFlags,
    }
};
use log::{debug, error, warn};
use rustix::fs::{Dev, major, makedev, minor};
use scopeguard::{guard, ScopeGuard};

use super::{GpuDevice, GpuInstance, has_extension};

struct PhysdevInfo {
    physdev: PhysicalDevice,
    props: PhysicalDeviceProperties,
    extensions: Extensions,
    primary: Option<Dev>,
    render: Option<Dev>,
    dmabuf_dev: Option<Dev>,
    score: u32,
}

const SCORE_MATCHES_DRM_DEV: u32 = 1 << 3;
const SCORE_DISCRETE_GPU: u32 = 1 << 2;
const SCORE_INTEGRATED_GPU: u32 = 1 << 1;
const SCORE_VIRTUAL_GPU: u32 = 1 << 0;

pub unsafe fn device(
    gpu_instance: &Rc<GpuInstance>,
    dmabuf_drm_dev: Option<Dev>,
) -> anyhow::Result<GpuDevice> {
    let instance = &gpu_instance.instance;
    let physdevs = instance.enumerate_physical_devices()
        .context("Failed to enumerate physical devices")?;
    let count = physdevs.len();
    if count == 0 {
        bail!("No physical devices found. Make sure you have a Vulkan driver \
            installed for your GPU and this application has permisson to \
            access graphics devices");
    }
    let mut physdev_infos = physdevs.into_iter()
        .filter_map(|physdev| physdev_info(instance, dmabuf_drm_dev, physdev))
        .collect::<Vec<_>>();
    physdev_infos.sort_by_key(|info| u32::MAX - info.score);
    let Some(max_score) = physdev_infos.first().map(|info| info.score) else {
        bail!("No physical devices could be probed")
    };
    physdev_infos.retain(|info| info.score == max_score);
    if physdev_infos.len() == 1 {
        debug!("Probed {} physical device(s), max score {}", count, max_score);
        return device_with_physdev(gpu_instance, physdev_infos.pop().unwrap())
    }
    warn!("Filtered multiple physical devices, {} out of {} with max score {}",
        physdev_infos.len(), count, max_score);
    let mut gpu_device_ok = None;
    let mut errors = Vec::new();
    for physdev_info in physdev_infos {
        match device_with_physdev(gpu_instance, physdev_info) {
            Ok(gpu_device) => {
                gpu_device_ok = Some(gpu_device);
                break
            },
            Err(e) => errors.push(e),
        }
    }
    if let Some(gpu_device) = gpu_device_ok {
        for e in errors {
            warn!("{e:#}");
        }
        Ok(gpu_device)
    } else {
        for e in errors {
            error!("{e:#}");
        }
        bail!("Failed to set up device with all filtered physical devices");
    }
}

unsafe fn physdev_info(
    instance: &Instance,
    dmabuf_dev: Option<Dev>,
    physdev: PhysicalDevice,
) -> Option<PhysdevInfo> {
    let extension_props_vec = match instance
        .enumerate_device_extension_properties(physdev)
    {
        Ok(ext_props) => ext_props,
        Err(e) => {
            let props = instance.get_physical_device_properties(physdev);
            let name = props.device_name_as_c_str().unwrap_or(c"unknown");
            let typ = props.device_type;
            error!("Failed to enumerate device extension properties
                for physical device {name:?} (type {typ:?}): {e}");
            return None
        }
    };
    let extensions = Extensions::new(extension_props_vec);
    let mut props2_chain = PhysicalDeviceProperties2::default();
    let has_drm_props = extensions.has(physical_device_drm::NAME);
    let mut drm_props = PhysicalDeviceDrmPropertiesEXT::default();
    if has_drm_props {
        props2_chain = props2_chain.push_next(&mut drm_props);
    }
    let has_driver_props = extensions.has(driver_properties::NAME);
    let mut driver_props = PhysicalDeviceDriverProperties::default();
    if has_driver_props {
        props2_chain = props2_chain.push_next(&mut driver_props);
    }
    instance.get_physical_device_properties2(physdev, &mut props2_chain);
    let props = props2_chain.properties;
    let name = props.device_name_as_c_str().unwrap_or(c"unknown");
    let typ = props.device_type;
    debug!("Probing physical device {name:?} (type {typ:?})");
    let mut score = 0u32;
    match typ {
        PhysicalDeviceType::DISCRETE_GPU => score |= SCORE_DISCRETE_GPU,
        PhysicalDeviceType::INTEGRATED_GPU => score |= SCORE_INTEGRATED_GPU,
        PhysicalDeviceType::VIRTUAL_GPU => score |= SCORE_VIRTUAL_GPU,
        _ => (),
    }
    if has_driver_props {
        debug!("Physical device driver: {:?}, {:?}",
            driver_props.driver_name_as_c_str().unwrap_or(c"unknown"),
            driver_props.driver_info_as_c_str().unwrap_or(c"unknown"));
    } else {
        debug!("VK_KHR_driver_properties unavailable");
    }
    let (mut primary, mut render) = (None, None);
    if has_drm_props {
        if drm_props.has_primary == vk::TRUE {
            primary = Some(makedev(
                drm_props.primary_major as _,
                drm_props.primary_minor as _,
            ));
        }
        if drm_props.has_render == vk::TRUE {
            render = Some(makedev(
                drm_props.render_major as _,
                drm_props.render_minor as _,
            ));
        }
        debug!("Physical device DRM devs: primary {}, render {}",
            fmt_dev_option(primary), fmt_dev_option(render));
        // Note [1]
        if dmabuf_dev.is_some()
            && (dmabuf_dev == primary || dmabuf_dev == render)
        {
            score |= SCORE_MATCHES_DRM_DEV;
            debug!("Physical device matched with the DMA-BUF feedback DRM dev");
        } else {
            debug!("Could not match physical device with the DMA-BUF feedback \
                DRM dev");
        }
    } else {
        debug!("VK_EXT_physical_device_drm unavailable");
    }
    Some(PhysdevInfo {
        physdev,
        props,
        extensions,
        primary,
        render,
        dmabuf_dev,
        score,
    })
}

unsafe fn device_with_physdev(
    gpu_instance: &Rc<GpuInstance>,
    physdev_info: PhysdevInfo,
) -> anyhow::Result<GpuDevice> {
    let name = physdev_info.props
        .device_name_as_c_str().unwrap_or(c"unknown").to_owned();
    let typ = physdev_info.props.device_type;
    let score = physdev_info.score;
    debug!("Setting up device with physical device {name:?} (type {typ:?})");
    let gpu_device = try_device_with_physdev(gpu_instance, physdev_info)
        .with_context(|| format!(
            "Failed to set up device with physical device {:?} (type {:?})",
            name, typ
        ))?;
    if score & SCORE_MATCHES_DRM_DEV == 0 {
        // We get here if either
        //  - using Wayland protocol Linux DMA-BUF version < 4
        //  - device has no VK_EXT_physical_device_drm
        warn!("IMPORTANT: Failed to ensure that we select the same GPU where \
            the compositor is running based on the DRM device numbers. About \
            to use physical device {:?} (type {:?}). If this is incorrect \
            then please restart without the --gpu option and open an issue",
            name, typ);
    }
    Ok(gpu_device)
}

unsafe fn try_device_with_physdev(
    gpu_instance: &Rc<GpuInstance>,
    physdev_info: PhysdevInfo,
) -> anyhow::Result<GpuDevice> {
    let instance = &gpu_instance.instance;
    let PhysdevInfo {
        physdev,
        props,
        mut extensions,
        primary,
        render,
        dmabuf_dev,
        ..
    } = physdev_info;
    let variant = api_version_variant(props.api_version);
    let major = api_version_major(props.api_version);
    let minor = api_version_minor(props.api_version);
    let patch = api_version_patch(props.api_version);
    if variant != 0 || major != 1 || minor < 1 {
        bail!("Need Vulkan device variant 0 version 1.1.0 or compatible,
            found variant {variant} version {major}.{minor}.{patch}");
    }
    debug!("Vulkan device supports version {major}.{minor}.{patch}");
    let memory_props = instance
        .get_physical_device_memory_properties(physdev);
    let queue_family_props = instance
        .get_physical_device_queue_family_properties(physdev);
    let queue_family_index = queue_family_props.iter()
        .position(|props| {
            props.queue_flags.contains(QueueFlags::GRAPHICS)
                && props.queue_count > 0
        })
        .context("Failed to find an appropriate queue family")? as u32;
    // Device extension dependency chains with Vulkan 1.1
    // app --> EXT_external_memory_dma_buf -> KHR_external_memory_fd
    //     \-> EXT_queue_family_foreign
    //     \-> (optional) EXT_image_drm_format_modifier -> KHR_image_format_list
    // EXT_image_drm_format_modifier is notably unsupported by
    //  - AMD GFX8 and older
    //  - end-of-life Nvidia GPUs which never got driver version 515
    extensions.try_enable(external_memory_fd::NAME)
        .context("KHR_external_memory_fd unavailable")?;
    extensions.try_enable(external_memory_dma_buf::NAME)
        .context("EXT_external_memory_dma_buf unavailable")?;
    extensions.try_enable(queue_family_foreign::NAME)
        .context("EXT_queue_family_foreign unavailable")?;
    extensions.try_enable(image_format_list::NAME)
        .context("KHR_image_format_list unavailable")?;
    let has_image_drm_format_modifier = extensions
        .try_enable(image_drm_format_modifier::NAME).is_some();
    let device = guard(
        instance.create_device(
            physdev,
            &DeviceCreateInfo::default()
                .queue_create_infos(&[DeviceQueueCreateInfo::default()
                    .queue_family_index(queue_family_index)
                    .queue_priorities(&[1.0])]
                )
                .enabled_extension_names(extensions.enabled()),
            None
        ).context("Failed to create device")?,
        |device| device.destroy_device(None),
    );
    let external_memory_fd_device =
        external_memory_fd::Device::new(instance, &device);
    let image_drm_format_modifier_device = if has_image_drm_format_modifier {
        Some(image_drm_format_modifier::Device::new(instance, &device))
    } else {
        debug!("EXT_image_drm_format_modifier unavailable");
        None
    };
    let queue = device.get_device_queue(queue_family_index, 0);
    let command_pool = guard(
        device.create_command_pool(
            &CommandPoolCreateInfo::default()
                .flags(CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                .queue_family_index(queue_family_index),
            None
        ).context("Failed to create command pool")?,
        |command_pool| device.destroy_command_pool(command_pool, None)
    );
    let command_buffer = guard(
        device.allocate_command_buffers(
            &CommandBufferAllocateInfo::default()
                .command_buffer_count(1)
                .command_pool(*command_pool)
                .level(CommandBufferLevel::PRIMARY)
        ).context("Failed to allocate command buffer")?[0],
        |command_buffer|
            device.free_command_buffers(*command_pool, &[command_buffer]),
    );
    let drm_format_props = if has_image_drm_format_modifier {
        Some(drm_format_props_b8g8r8a8_srgb(instance, physdev))
    } else {
        None
    };
    Ok(GpuDevice {
        command_buffer: ScopeGuard::into_inner(command_buffer),
        command_pool: ScopeGuard::into_inner(command_pool),
        device: ScopeGuard::into_inner(device),
        external_memory_fd_device,
        image_drm_format_modifier_device,
        physdev,
        memory_props,
        drm_format_props,
        primary_drm_dev: primary,
        render_drm_dev: render,
        dmabuf_drm_dev: dmabuf_dev,
        queue,
        queue_family_index,
        gpu_instance: Rc::clone(gpu_instance),
    })
}

struct Extensions {
    props: Vec<ExtensionProperties>,
    enabled: Vec<*const c_char>,
}

impl Extensions {
    fn new(props: Vec<ExtensionProperties>) -> Extensions {
        Extensions { props, enabled: Vec::new() }
    }

    fn has(&self, name: &CStr) -> bool {
        has_extension(&self.props, name)
    }

    fn try_enable(&mut self, extension: &CStr) -> Option<()> {
        if self.props.iter().any(|ext|
            ext.extension_name_as_c_str() == Ok(extension)
        ) {
            self.enabled.push(extension.as_ptr());
            Some(())
        } else {
            None
        }
    }

    fn enabled(&self) -> &[*const c_char] {
        &self.enabled
    }
}

fn fmt_dev_option(dev: Option<Dev>) -> String {
    if let Some(dev) = dev {
        format!("{}:{}", major(dev), minor(dev))
    } else {
        "unavailable".to_string()
    }
}

unsafe fn drm_format_props_b8g8r8a8_srgb(
    instance: &Instance,
    physdev: PhysicalDevice,
) -> Vec<DrmFormatModifierPropertiesEXT> {
    let mut drm_format_props_list =
        DrmFormatModifierPropertiesListEXT::default();
    instance.get_physical_device_format_properties2(
        physdev,
        Format::B8G8R8A8_SRGB,
        &mut FormatProperties2::default().push_next(&mut drm_format_props_list),
    );
    let mut drm_format_props = vec![
        DrmFormatModifierPropertiesEXT::default();
        drm_format_props_list.drm_format_modifier_count as usize
    ];
    drm_format_props_list = drm_format_props_list
        .drm_format_modifier_properties(&mut drm_format_props);
    let mut format_props_chain = FormatProperties2::default()
        .push_next(&mut drm_format_props_list);
    instance.get_physical_device_format_properties2(
        physdev,
        Format::B8G8R8A8_SRGB,
        &mut format_props_chain,
    );
    drm_format_props
}

// [1] Wayland DMA-BUF says for the feedback main device and for the tranche
// target device one must not compare the dev_t and should use drmDevicesEqual
// from libdrm.so instead to find the same GPU. But neither Mesa Vulkan WSI
// Wayland nor wlroots Vulkan renderer does that, they both use
// PhysicalDeviceDrmPropertiesEXT the same way we do here. So this is probably
// fine (because it provides both the primary and the render DRM node to
// compare against not just one of them (?)). Do we need a fallback using
// libdrm drmDevicesEqual?
