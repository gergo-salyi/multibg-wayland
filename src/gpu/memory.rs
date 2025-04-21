#![allow(clippy::too_many_arguments)]

use std::{
    os::fd::{FromRawFd, OwnedFd},
    rc::Rc,
    slice,
};

use anyhow::{bail, Context};
use ash::{
    Instance,
    vk::{
        AccessFlags,
        BufferCreateFlags,
        BufferCreateInfo,
        BufferImageCopy,
        BufferUsageFlags,
        CommandBufferBeginInfo,
        CommandBufferResetFlags,
        DependencyFlags,
        DeviceSize,
        DrmFormatModifierPropertiesEXT,
        ExportMemoryAllocateInfo,
        Extent2D,
        ExternalMemoryHandleTypeFlags,
        ExternalMemoryImageCreateInfo,
        Fence,
        Format,
        FormatFeatureFlags,
        ImageAspectFlags,
        ImageCreateFlags,
        ImageCreateInfo,
        ImageDrmFormatModifierListCreateInfoEXT,
        ImageDrmFormatModifierPropertiesEXT,
        ImageFormatProperties2,
        ImageLayout,
        ImageMemoryBarrier,
        ImageSubresource,
        ImageSubresourceLayers,
        ImageSubresourceRange,
        ImageTiling,
        ImageType,
        ImageUsageFlags,
        MemoryAllocateInfo,
        MemoryGetFdInfoKHR,
        MemoryMapFlags,
        MemoryPropertyFlags,
        MemoryRequirements,
        PhysicalDevice,
        PhysicalDeviceImageDrmFormatModifierInfoEXT,
        PhysicalDeviceImageFormatInfo2,
        PhysicalDeviceMemoryProperties,
        PipelineStageFlags,
        QUEUE_FAMILY_FOREIGN_EXT,
        SampleCountFlags,
        SharingMode,
        SubmitInfo,
    }
};
use log::debug;
use scopeguard::{guard, ScopeGuard};

use super::{
    DRM_FORMAT_MOD_LINEAR, fmt_modifier,
    GpuDevice, GpuMemory, GpuUploader, GpuWallpaper,
    MemoryPlane,
};

pub unsafe fn uploader(
    gpu_device: Rc<GpuDevice>,
    width: u32,
    height: u32,
    drm_format_modifiers: Vec<u64>,
) -> anyhow::Result<GpuUploader> {
    let GpuDevice {
        gpu_instance,
        memory_props,
        drm_format_props,
        device,
        ..
    } = gpu_device.as_ref();
    let instance = &gpu_instance.instance;
    let physdev = gpu_device.physdev;
    let queue_family_index = gpu_device.queue_family_index;
    let size = width as DeviceSize * height as DeviceSize * 4;
    let mut filtered_modifiers = Vec::with_capacity(drm_format_modifiers.len());
    if let Some(drm_format_props) = drm_format_props {
        for &drm_format_modifier in drm_format_modifiers.iter() {
            match filter_modifier(
                instance, physdev, queue_family_index, drm_format_props,
                width, height, size, drm_format_modifier,
            ) {
                Ok(()) => filtered_modifiers.push(drm_format_modifier),
                Err(e) => debug!("Cannot use DRM format modifier {}: {:#}",
                    fmt_modifier(drm_format_modifier), e),
            }
            if filtered_modifiers.is_empty() {
                bail!("None of the DRM format modifiers can be \
                    used for image creation");
            }
        }
    } else if drm_format_modifiers.contains(&DRM_FORMAT_MOD_LINEAR) {
        debug!("Image creation can only use DRM_FORMAT_MOD_LINEAR");
    } else {
        bail!("VK_EXT_physical_device_drm unavailable and \
            no DRM_FORMAT_MOD_LINEAR was proposed for image creation");
    }
    debug!("Image creation can use DRM format modifiers: {}",
        filtered_modifiers.iter()
            .map(|&modifier| fmt_modifier(modifier))
            .collect::<Vec<_>>().join(", "));
    let buffer = guard(
        device.create_buffer(
            &BufferCreateInfo::default()
                .flags(BufferCreateFlags::empty())
                .size(size)
                .usage(BufferUsageFlags::TRANSFER_SRC)
                .sharing_mode(SharingMode::EXCLUSIVE)
                .queue_family_indices(slice::from_ref(&queue_family_index)),
            None
        ).context("Failed to create staging buffer")?,
        |buffer| device.destroy_buffer(buffer, None),
    );
    let buffer_memory_req = device.get_buffer_memory_requirements(*buffer);
    let buffer_memory_index = find_memorytype_index(
        &buffer_memory_req,
        memory_props,
        MemoryPropertyFlags::HOST_VISIBLE | MemoryPropertyFlags::HOST_COHERENT
            | MemoryPropertyFlags::HOST_CACHED,
    ).context("Cannot find suitable device memory type for staging buffer")?;
    let memory = guard(
        device.allocate_memory(
            &MemoryAllocateInfo::default()
                .allocation_size(buffer_memory_req.size)
                .memory_type_index(buffer_memory_index),
            None
        ).context("Failed to allocate memory for staging buffer")?,
        |memory| device.free_memory(memory, None),
    );
    device.bind_buffer_memory(*buffer, *memory, 0)
        .context("Failed to bind memory to staging buffer")?;
    let ptr = device.map_memory(
        *memory,
        0,
        buffer_memory_req.size,
        MemoryMapFlags::empty()
    ).context("Failed to map staging buffer memory")?;
    Ok(GpuUploader {
        memory: ScopeGuard::into_inner(memory),
        buffer: ScopeGuard::into_inner(buffer),
        ptr: ptr.cast(),
        len: buffer_memory_req.size as usize,
        extent: Extent2D { width, height },
        drm_format_modifiers: filtered_modifiers,
        gpu_device,
    })
}

unsafe fn filter_modifier(
    instance: &Instance,
    physdev: PhysicalDevice,
    queue_family_index: u32,
    drm_format_props: &[DrmFormatModifierPropertiesEXT],
    width: u32,
    height: u32,
    size: DeviceSize,
    drm_format_modifier: u64,
) -> anyhow::Result<()> {
    let format_props = drm_format_props.iter()
        .find(|props| props.drm_format_modifier == drm_format_modifier)
        .context("This modifier is unsupported by this Vulkan context")?;
    if !format_props.drm_format_modifier_tiling_features
        .contains(FormatFeatureFlags::TRANSFER_DST)
    {
        bail!("FormatFeatureFlag TRANSFER_DST unsupported");
    }
    let mut image_format_props2 = ImageFormatProperties2::default();
    let mut image_drm_info =
        PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(drm_format_modifier)
            .sharing_mode(SharingMode::EXCLUSIVE)
            .queue_family_indices(slice::from_ref(&queue_family_index));
    instance.get_physical_device_image_format_properties2(
        physdev,
        &PhysicalDeviceImageFormatInfo2::default()
            .format(Format::B8G8R8A8_SRGB)
            .ty(ImageType::TYPE_2D)
            .tiling(ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(ImageUsageFlags::TRANSFER_DST)
            .flags(ImageCreateFlags::empty())
            .push_next(&mut image_drm_info),
        &mut image_format_props2,
    ).context("The needed image format is unsupported for this modifier")?;
    let image_format_props = image_format_props2.image_format_properties;
    if image_format_props.max_extent.depth < 1
        || image_format_props.max_mip_levels < 1
        || image_format_props.max_array_layers < 1
        || !image_format_props.sample_counts.contains(SampleCountFlags::TYPE_1)
    {
        bail!("The needed image format is unsupported for this modifier")
    }
    let max_width = image_format_props.max_extent.width;
    let max_height = image_format_props.max_extent.width;
    let max_size = image_format_props.max_resource_size;
    if width > max_width {
        bail!("Needed image width {width} is greter then the max supported \
            image width {max_width}")
    }
    if height > max_height {
        bail!("Needed image height {height} is greter then the max supported \
            image height {max_height}")
    }
    if size > max_size {
        bail!("Needed image size {size} bytes is greter then the max supported \
            image size {max_width} bytes")
    }
    Ok(())
}

// XXX: we could check if dedicated allocation is needed:
// https://registry.khronos.org/vulkan/specs/latest/man/html/VK_KHR_dedicated_allocation.html
pub unsafe fn upload(
    uploader: &mut GpuUploader,
) -> anyhow::Result<GpuWallpaper> {
    let GpuUploader {
        gpu_device,
        drm_format_modifiers,
        ..
    } = uploader;
    let GpuDevice {
        memory_props,
        drm_format_props,
        device,
        external_memory_fd_device,
        image_drm_format_modifier_device,
        ..
    } = gpu_device.as_ref();
    let extent = uploader.extent;
    let buffer = uploader.buffer;
    let queue_family_index = gpu_device.queue_family_index;
    let command_buffer = gpu_device.command_buffer;
    let queue = gpu_device.queue;
    let mut external_memory_info = ExternalMemoryImageCreateInfo::default()
        .handle_types(ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let mut modifier_list_info =
        ImageDrmFormatModifierListCreateInfoEXT::default()
            .drm_format_modifiers(drm_format_modifiers);
    let mut image_create_info = ImageCreateInfo::default()
        .flags(ImageCreateFlags::empty())
        .image_type(ImageType::TYPE_2D)
        .format(Format::B8G8R8A8_SRGB)
        .extent(extent.into())
        .mip_levels(1)
        .array_layers(1)
        .samples(SampleCountFlags::TYPE_1)
        .usage(ImageUsageFlags::TRANSFER_DST)
        .sharing_mode(SharingMode::EXCLUSIVE)
        .queue_family_indices(slice::from_ref(&queue_family_index))
        .initial_layout(ImageLayout::UNDEFINED)
        .push_next(&mut external_memory_info);
    if image_drm_format_modifier_device.is_some() {
        image_create_info = image_create_info
            .tiling(ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .push_next(&mut modifier_list_info);
    } else {
        image_create_info = image_create_info.tiling(ImageTiling::LINEAR);
    }
    let image = guard(
        device.create_image(&image_create_info, None)
            .context("Failed to create image")?,
        |image| device.destroy_image(image, None),
    );
    let image_memory_req = device.get_image_memory_requirements(*image);
    let image_memory_index = find_memorytype_index(
        &image_memory_req,
        memory_props,
        MemoryPropertyFlags::DEVICE_LOCAL,
    ).context("Failed to find memorytype index for image")?;
    let image_memory = guard(
        device.allocate_memory(
            &MemoryAllocateInfo::default()
                .allocation_size(image_memory_req.size)
                .memory_type_index(image_memory_index)
                .push_next(&mut ExportMemoryAllocateInfo::default()
                    .handle_types(ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                ),
            None
        ).context("Failed to allocate memory for image")?,
        |memory| device.free_memory(memory, None),
    );
    device.bind_image_memory(*image, *image_memory, 0)
        .context("Failed to bind image memory")?;
    device.reset_command_buffer(
        command_buffer,
        CommandBufferResetFlags::empty()
    ).context("Failed to reset command buffer")?;
    device.begin_command_buffer(
        command_buffer,
        &CommandBufferBeginInfo::default()
    ) .context("Failed to begin command buffer")?;
    device.cmd_pipeline_barrier(
        command_buffer,
        PipelineStageFlags::TOP_OF_PIPE,
        PipelineStageFlags::TRANSFER,
        DependencyFlags::empty(),
        &[],
        &[],
        &[ImageMemoryBarrier::default()
            .src_access_mask(AccessFlags::NONE)
            .dst_access_mask(AccessFlags::TRANSFER_WRITE)
            .old_layout(ImageLayout::UNDEFINED)
            .new_layout(ImageLayout::GENERAL)
            .src_queue_family_index(queue_family_index)
            .dst_queue_family_index(queue_family_index)
            .image(*image)
            .subresource_range(ImageSubresourceRange::default()
                .aspect_mask(ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1)
            )
        ],
    );
    device.cmd_copy_buffer_to_image(
        command_buffer,
        buffer,
        *image,
        ImageLayout::GENERAL,
        &[BufferImageCopy::default()
            .image_subresource(ImageSubresourceLayers::default()
                .aspect_mask(ImageAspectFlags::COLOR)
                .layer_count(1)
            )
            .image_extent(extent.into())
        ]
    );
    // https://registry.khronos.org/vulkan/specs/latest/html/vkspec.html#resources-external-sharing
    device.cmd_pipeline_barrier(
        command_buffer,
        PipelineStageFlags::TRANSFER,
        PipelineStageFlags::BOTTOM_OF_PIPE,
        DependencyFlags::empty(),
        &[],
        &[],
        &[ImageMemoryBarrier::default()
            .src_access_mask(AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(AccessFlags::NONE)
            .old_layout(ImageLayout::GENERAL)
            .new_layout(ImageLayout::GENERAL)
            .src_queue_family_index(queue_family_index)
            .dst_queue_family_index(QUEUE_FAMILY_FOREIGN_EXT)
            .image(*image)
            .subresource_range(ImageSubresourceRange::default()
                .aspect_mask(ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1)
            )
        ],
    );
    device.end_command_buffer(command_buffer)
        .context("Failed to end command buffer")?;
    device.queue_submit(
        queue,
        &[SubmitInfo::default().command_buffers(&[command_buffer])],
        Fence::null(),
    ).context("Failed to submit queue")?;
    device.queue_wait_idle(queue).context("Failed to wait queue idle")?;
    let mut drm_format_modifier = DRM_FORMAT_MOD_LINEAR;
    let mut memory_plane_count = 1;
    let mut aspect_masks = [ImageAspectFlags::COLOR; 4];
    if let Some(modifier_device) = image_drm_format_modifier_device {
        let mut props = ImageDrmFormatModifierPropertiesEXT::default();
        modifier_device
            .get_image_drm_format_modifier_properties(*image, &mut props)
            .context("Failed to get image drm format modifier properties")?;
        drm_format_modifier = props.drm_format_modifier;
        debug!("Image created with DRM format modifier {}",
            fmt_modifier(drm_format_modifier));
        let format_prop = drm_format_props.as_ref().unwrap().iter().find(|f|
            f.drm_format_modifier == drm_format_modifier
        ).context("Failed to find DRM format modifier properties")?;
        memory_plane_count = format_prop
            .drm_format_modifier_plane_count as usize;
        aspect_masks = [
            ImageAspectFlags::MEMORY_PLANE_0_EXT,
            ImageAspectFlags::MEMORY_PLANE_1_EXT,
            ImageAspectFlags::MEMORY_PLANE_2_EXT,
            ImageAspectFlags::MEMORY_PLANE_3_EXT,
        ];
    }
    let mut memory_planes = [MemoryPlane::default(); 4];
    for memory_plan_index in 0..memory_plane_count {
        let subresource_layout = device.get_image_subresource_layout(
            *image,
            ImageSubresource::default()
                .aspect_mask(aspect_masks[memory_plan_index])
                .mip_level(0)
                .array_layer(0)
        );
        memory_planes[memory_plan_index] = MemoryPlane {
            offset: subresource_layout.offset,
            stride: subresource_layout.row_pitch,
        };
    }
    let raw_fd = external_memory_fd_device.get_memory_fd(
        &MemoryGetFdInfoKHR::default()
            .memory(*image_memory)
            .handle_type(ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
    ).context("Failed to get memory fd")?;
    if raw_fd < 0 {
        bail!("Got invalid memory fd {raw_fd}")
    }
    let fd = OwnedFd::from_raw_fd(raw_fd);
    Ok(GpuWallpaper {
        drm_format_modifier,
        memory_planes_len: memory_plane_count,
        memory_planes,
        gpu_memory: GpuMemory {
            memory: ScopeGuard::into_inner(image_memory),
            size: uploader.len,
            image: ScopeGuard::into_inner(image),
            gpu_device: Rc::clone(&uploader.gpu_device),
            drm_format_modifier,
        },
        fd,
    })
}

fn find_memorytype_index(
    memory_req: &MemoryRequirements,
    memory_prop: &PhysicalDeviceMemoryProperties,
    flags: MemoryPropertyFlags,
) -> Option<u32> {
    memory_prop.memory_types[..memory_prop.memory_type_count as _]
        .iter()
        .enumerate()
        .find(|(index, memory_type)| {
            (1 << index) & memory_req.memory_type_bits != 0
                && memory_type.property_flags & flags == flags
        })
        .map(|(index, _memory_type)| index as _)
}
