use std::{
    backtrace::Backtrace,
    borrow::Cow,
    ffi::{c_char, c_void, CStr},
    ptr,
};

use anyhow::{bail, Context};
use ash::{
    Entry,
    ext::{
        debug_report,
        debug_utils,
    },
    vk::{
        self,
        api_version_variant,
        api_version_major,
        api_version_minor,
        api_version_patch,
        ApplicationInfo,
        Bool32,
        DebugReportCallbackCreateInfoEXT,
        DebugReportFlagsEXT,
        DebugReportObjectTypeEXT,
        DebugUtilsMessengerCallbackDataEXT,
        DebugUtilsMessengerCreateInfoEXT,
        DebugUtilsMessageSeverityFlagsEXT,
        DebugUtilsMessageTypeFlagsEXT,
        InstanceCreateInfo,
        LayerProperties,
        make_api_version,
    }
};
use log::{debug, error, info, warn};

use super::{Debug, GpuInstance, has_extension};

const APP_VK_NAME: &CStr = match CStr::from_bytes_with_nul(
    concat!(env!("CARGO_PKG_NAME"), '\0').as_bytes()
) {
    Ok(val) => val,
    Err(_) => panic!(),
};
const APP_VK_VERSION: u32 = make_api_version(
    0,
    parse_decimal(env!("CARGO_PKG_VERSION_MAJOR")),
    parse_decimal(env!("CARGO_PKG_VERSION_MINOR")),
    parse_decimal(env!("CARGO_PKG_VERSION_PATCH")),
);
const LAYER_VALIDATION: &CStr = c"VK_LAYER_KHRONOS_validation";
const VULKAN_VERSION_TARGET: u32 = make_api_version(0, 1, 1, 0);

pub unsafe fn instance() -> anyhow::Result<GpuInstance> {
    let entry = Entry::load()
        .context("Failed to load Vulkan shared libraries. Make sure you have \
            the Vulkan loader and a Vulkan driver for your GPU installed")?;
    let instance_version = entry.try_enumerate_instance_version()
        .context("Failed to enumerate instance version")?
        .unwrap_or_else(|| make_api_version(0, 1, 0, 0));
    let variant = api_version_variant(instance_version);
    let major = api_version_major(instance_version);
    let minor = api_version_minor(instance_version);
    let patch = api_version_patch(instance_version);
    if variant != 0 || major != 1 || minor < 1 {
        bail!("Need Vulkan instance variant 0 version 1.1.0 or compatible,
            found variant {variant} version {major}.{minor}.{patch}");
    }
    debug!("Vulkan instance supports version {major}.{minor}.{patch}");
    let instance_layer_props = entry.enumerate_instance_layer_properties()
        .context("Failed to enumerate instance layers")?;
    let instance_extension_props = entry
        .enumerate_instance_extension_properties(None)
        .context("Failed to enumerate instance extensions")?;
    let mut instance_layers = Vec::new();
    let mut extension_layers = Vec::new();
    let mut has_debug_utils = false;
    let mut has_debug_report = false;
    if log::log_enabled!(log::Level::Debug) {
        debug!("Running with log level DEBUG or higher \
            so trying to enable Vulkan validation layers");
        if has_layer(&instance_layer_props, LAYER_VALIDATION) {
            instance_layers.push(LAYER_VALIDATION.as_ptr());
            info!("Enabling VK_LAYER_KHRONOS_validation");
        } else {
            warn!("VK_LAYER_KHRONOS_validation unavailable");
        }
        if has_extension(&instance_extension_props, debug_utils::NAME) {
            debug!("Enabling VK_EXT_debug_utils");
            has_debug_utils = true;
            extension_layers.push(debug_utils::NAME.as_ptr());
        } else if has_extension(&instance_extension_props, debug_report::NAME) {
            debug!("Enabling VK_EXT_debug_report");
            has_debug_report = true;
            extension_layers.push(debug_report::NAME.as_ptr());
        } else {
            warn!("VK_EXT_debug_utils and VK_EXT_debug_report unavailable");
        }
    }
    let instance = entry.create_instance(
        &InstanceCreateInfo::default()
            .application_info(&ApplicationInfo::default()
                .application_name(APP_VK_NAME)
                .application_version(APP_VK_VERSION)
                .engine_name(APP_VK_NAME)
                .engine_version(APP_VK_VERSION)
                .api_version(VULKAN_VERSION_TARGET)
            )
            .enabled_layer_names(&instance_layers)
            .enabled_extension_names(&extension_layers),
        None,
    ).context("Failed to create instance")?;
    let mut debug = Debug::None;
    if has_debug_utils {
        let instance = debug_utils::Instance::new(&entry, &instance);
        match instance.create_debug_utils_messenger(
            &DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    DebugUtilsMessageSeverityFlagsEXT::ERROR
                    | DebugUtilsMessageSeverityFlagsEXT::WARNING
                )
                .message_type(
                    DebugUtilsMessageTypeFlagsEXT::GENERAL
                    | DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_utils_callback)),
            None
        ) {
            Ok(messenger) =>
                debug = Debug::Utils { instance, messenger },
            Err(e) =>
                error!("Failed to create Vulkan debug utils messenger: {e}"),
        };
    } else if has_debug_report {
        let instance = debug_report::Instance::new(&entry, &instance);
        #[allow(deprecated)]
        match instance.create_debug_report_callback(
            &DebugReportCallbackCreateInfoEXT::default()
                .flags(DebugReportFlagsEXT::WARNING
                    | DebugReportFlagsEXT::PERFORMANCE_WARNING
                    | DebugReportFlagsEXT::ERROR)
                .pfn_callback(Some(debug_report_callback))
                .user_data(ptr::null_mut()),
            None,
        ) {
            Ok(callback) =>
                debug = Debug::Report { instance, callback },
            Err(e) =>
                error!("Failed to create Vulkan debug report callback: {e}"),
        };
    }
    Ok(GpuInstance {
        _entry: entry,
        instance,
        debug,
    })
}

const fn parse_decimal(src: &str) -> u32 {
    match u32::from_str_radix(src, 10) {
        Ok(val) => val,
        Err(_) => panic!(),
    }
}

fn has_layer(layers: &[LayerProperties], name: &CStr) -> bool {
    layers.iter().any(|layer| layer.layer_name_as_c_str() == Ok(name))
}

unsafe extern "system" fn debug_utils_callback(
    message_severity: DebugUtilsMessageSeverityFlagsEXT,
    message_type: DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> Bool32 {
    if p_callback_data.is_null() {
        error!("Vulkan: null data");
        return vk::FALSE
    }
    let callback_data = *p_callback_data;
    let message = if callback_data.p_message.is_null() {
        Cow::from("null message")
    } else {
        CStr::from_ptr(callback_data.p_message).to_string_lossy()
    };
    match message_severity {
        DebugUtilsMessageSeverityFlagsEXT::ERROR => {
            let backtrace = Backtrace::force_capture();
            error!("Vulkan: {message}\nBacktrace:\n{backtrace}");
        },
        DebugUtilsMessageSeverityFlagsEXT::WARNING => {
            warn!("Vulkan: {message}");
        },
        severity => {
            error!("Unexpected Vulkan message {:?} {:?}: {}",
                message_type, severity, message);
        },
    };
    vk::FALSE
}

unsafe extern "system" fn debug_report_callback(
    flags: DebugReportFlagsEXT,
    _object_type: DebugReportObjectTypeEXT,
    _object: u64,
    _location: usize,
    _message_code: i32,
    _p_layer_prefix: *const c_char,
    p_message: *const c_char,
    _p_user_data: *mut c_void
) -> u32 {
    let message = if p_message.is_null() {
        Cow::from("null message")
    } else {
        CStr::from_ptr(p_message).to_string_lossy()
    };
    match flags {
        DebugReportFlagsEXT::ERROR => {
            let backtrace = Backtrace::force_capture();
            error!("Vulkan: {message}\nBacktrace:\n{backtrace}");
        },
        DebugReportFlagsEXT::WARNING
        | DebugReportFlagsEXT::PERFORMANCE_WARNING => {
            warn!("Vulkan: {message}");
        },
        flag => {
            error!("Unexpected Vulkan message {flag:?}: {message}");
        }
    };
    vk::FALSE
}
