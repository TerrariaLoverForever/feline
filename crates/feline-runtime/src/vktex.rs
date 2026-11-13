// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! ETC2 textures on desktop Vulkan.
//!
//! Roblox's Android app picks its texture format from what the GPU can
//! sample. Under OpenGL ES the desktop drivers offer ETC2, and the app
//! downloads compact, full-quality ETC2 textures. Under Vulkan they usually
//! don't (desktop GPUs have BC formats instead), and the app falls back to
//! PNG textures, which it keeps at lower detail: the blurry look.
//!
//! So Feline's Vulkan layer says ETC2 is available (the
//! `textureCompressionETC2` feature and the ETC2/EAC formats' properties),
//! creates the engine's ETC2 images as plain RGBA8 (EAC: R16/RG16), and
//! when the engine records a buffer → image upload into one, decodes the
//! blocks on the CPU (etc2.rs) into Feline's own staging memory and records
//! that upload instead. The engine writes a texture's data before it records
//! the copy, as Vulkan's usual staging pattern does.
//!
//! Feline's staging memory is reused once the GPU is surely done with it:
//! a while (frames and time) after the last upload recorded into it.
//!
//! `FELINE_ETC2=0` turns this off (the engine then uses PNG textures).
//!
//! # `unsafe` in this module
//!
//! Vulkan's C API with the engine's arguments, and reading the engine's
//! mapped staging memory.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use ash::vk::{self, Handle};

use crate::etc2::Kind;

/// Whether ETC2 is offered (on unless FELINE_ETC2=0).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("FELINE_ETC2").map_or(true, |v| v != "0");
        if on {
            eprintln!("feline-runtime: vulkan: ETC2 textures offered (decoded by Feline)");
        }
        on
    })
}

/// The ETC2/EAC `format`'s kind and the format Feline makes instead.
fn substitute(format: vk::Format) -> Option<(Kind, vk::Format)> {
    Kind::of(format.as_raw()).map(|(kind, plain)| (kind, vk::Format::from_raw(plain)))
}

/// What a decoded ETC2 image can do (what the engine needs of a texture).
const KEEP: vk::FormatFeatureFlags = vk::FormatFeatureFlags::from_raw(
    vk::FormatFeatureFlags::SAMPLED_IMAGE.as_raw()
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR.as_raw()
        | vk::FormatFeatureFlags::TRANSFER_SRC.as_raw()
        | vk::FormatFeatureFlags::TRANSFER_DST.as_raw()
        | vk::FormatFeatureFlags::BLIT_SRC.as_raw(),
);

// ---------------------------------------------------------------------------
// The driver's function tables
// ---------------------------------------------------------------------------

/// The driver's tables for the engine's current instance.
fn instance_table() -> Option<ash::Instance> {
    static TABLE: Mutex<Option<(u64, ash::Instance)>> = Mutex::new(None);
    let instance = crate::vulkan::current_instance();
    if instance == 0 {
        return None;
    }
    let mut slot = TABLE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((i, table)) = slot.as_ref()
        && *i == instance
    {
        return Some(table.clone());
    }
    let gipa = crate::vulkan::host_get_instance_proc_addr()?;
    // SAFETY: the driver's vkGetInstanceProcAddr (C and "system" are the
    // same ABI on x86_64 Linux).
    let static_fn = ash::StaticFn {
        get_instance_proc_addr: unsafe { std::mem::transmute::<usize, vk::PFN_vkGetInstanceProcAddr>(gipa) },
    };
    // SAFETY: the engine's live instance, made through the same driver.
    let table = unsafe { ash::Instance::load(&static_fn, vk::Instance::from_raw(instance)) };
    *slot = Some((instance, table.clone()));
    Some(table)
}

/// Feline's state for the engine's device.
struct Dev {
    handle: u64,
    table: ash::Device,
    memory_props: vk::PhysicalDeviceMemoryProperties,
    /// ETC2 images: image → its kind.
    images: HashMap<u64, Kind>,
    /// Buffers: buffer → (memory, offset it's bound at).
    buffers: HashMap<u64, (u64, u64)>,
    /// Mapped memory: memory → (offset mapped from, address).
    maps: HashMap<u64, (u64, usize)>,
    staging: Vec<Chunk>,
    /// The chunk being filled.
    active: Option<usize>,
    told_unmapped: bool,
}

// SAFETY: the raw addresses are only used under STATE's lock.
unsafe impl Send for Dev {}

static STATE: Mutex<Option<Dev>> = Mutex::new(None);

fn with_dev<T>(f: impl FnOnce(&mut Dev) -> T) -> Option<T> {
    let mut guard = STATE.lock().unwrap_or_else(|p| p.into_inner());
    guard.as_mut().map(f)
}

/// Frames presented (for reusing staging memory).
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// After each present.
pub fn frame_presented() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// The engine made `device` on `physical`.
pub fn device_made(device: u64, physical: u64) {
    if !enabled() {
        return;
    }
    let Some(instance) = instance_table() else {
        return;
    };
    // SAFETY: the engine's live device, made through the same driver.
    let table = unsafe { ash::Device::load(instance.fp_v1_0(), vk::Device::from_raw(device)) };
    let physical = vk::PhysicalDevice::from_raw(physical);
    // SAFETY: a live physical device.
    let memory_props = unsafe { instance.get_physical_device_memory_properties(physical) };
    let mut guard = STATE.lock().unwrap_or_else(|p| p.into_inner());
    // A previous device's state is stale (the engine destroyed it).
    *guard = Some(Dev {
        handle: device,
        table,
        memory_props,
        images: HashMap::new(),
        buffers: HashMap::new(),
        maps: HashMap::new(),
        staging: Vec::new(),
        active: None,
        told_unmapped: false,
    });
}

/// Clears ETC2 from the features the engine asks the driver for in
/// `vkCreateDevice` (the driver doesn't have it; Feline does).
///
/// # Safety
/// `features` is NULL or the engine's `VkPhysicalDeviceFeatures`; `next` is
/// the engine's valid `pNext` chain.
pub unsafe fn strip_device_features(features: *const c_void, next: *const c_void) -> Option<vk::PhysicalDeviceFeatures> {
    if !enabled() {
        return None;
    }
    // Features 2 in the chain: changed in place (the engine's own struct,
    // only read by the driver during this call).
    let mut at = next.cast::<vk::BaseOutStructure>().cast_mut();
    while !at.is_null() {
        // SAFETY: a valid chain of Vulkan structures.
        unsafe {
            if (*at).s_type == vk::StructureType::PHYSICAL_DEVICE_FEATURES_2 {
                (*at.cast::<vk::PhysicalDeviceFeatures2>()).features.texture_compression_etc2 = vk::FALSE;
            }
            at = (*at).p_next;
        }
    }
    if features.is_null() {
        return None;
    }
    // SAFETY: the engine's valid struct.
    let mut copy = unsafe { *features.cast::<vk::PhysicalDeviceFeatures>() };
    copy.texture_compression_etc2 = vk::FALSE;
    Some(copy)
}

// ---------------------------------------------------------------------------
// The functions Feline answers
// ---------------------------------------------------------------------------

macro_rules! ours {
    ($f:expr) => {
        Some(unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn()>($f as *const ()) })
    };
}

/// Feline's version of the Vulkan function `name`, if it has one.
pub fn hook(name: &[u8]) -> Option<unsafe extern "C" fn()> {
    if !enabled() {
        return None;
    }
    match name {
        b"vkGetPhysicalDeviceFeatures" => ours!(get_features as vk::PFN_vkGetPhysicalDeviceFeatures),
        b"vkGetPhysicalDeviceFeatures2" | b"vkGetPhysicalDeviceFeatures2KHR" => {
            ours!(get_features2 as vk::PFN_vkGetPhysicalDeviceFeatures2)
        }
        b"vkGetPhysicalDeviceFormatProperties" => {
            ours!(get_format_properties as vk::PFN_vkGetPhysicalDeviceFormatProperties)
        }
        b"vkGetPhysicalDeviceFormatProperties2" | b"vkGetPhysicalDeviceFormatProperties2KHR" => {
            ours!(get_format_properties2 as vk::PFN_vkGetPhysicalDeviceFormatProperties2)
        }
        b"vkGetPhysicalDeviceImageFormatProperties" => {
            ours!(get_image_format_properties as vk::PFN_vkGetPhysicalDeviceImageFormatProperties)
        }
        b"vkGetPhysicalDeviceImageFormatProperties2" | b"vkGetPhysicalDeviceImageFormatProperties2KHR" => {
            ours!(get_image_format_properties2 as vk::PFN_vkGetPhysicalDeviceImageFormatProperties2)
        }
        b"vkCreateImage" => ours!(create_image as vk::PFN_vkCreateImage),
        b"vkDestroyImage" => ours!(destroy_image as vk::PFN_vkDestroyImage),
        b"vkCreateImageView" => ours!(create_image_view as vk::PFN_vkCreateImageView),
        b"vkBindBufferMemory" => ours!(bind_buffer_memory as vk::PFN_vkBindBufferMemory),
        b"vkBindBufferMemory2" | b"vkBindBufferMemory2KHR" => {
            ours!(bind_buffer_memory2 as vk::PFN_vkBindBufferMemory2)
        }
        b"vkDestroyBuffer" => ours!(destroy_buffer as vk::PFN_vkDestroyBuffer),
        b"vkMapMemory" => ours!(map_memory as vk::PFN_vkMapMemory),
        b"vkUnmapMemory" => ours!(unmap_memory as vk::PFN_vkUnmapMemory),
        b"vkFreeMemory" => ours!(free_memory as vk::PFN_vkFreeMemory),
        b"vkCmdCopyBufferToImage" => ours!(cmd_copy_buffer_to_image as vk::PFN_vkCmdCopyBufferToImage),
        b"vkCmdCopyBufferToImage2" | b"vkCmdCopyBufferToImage2KHR" => {
            ours!(cmd_copy_buffer_to_image2 as vk::PFN_vkCmdCopyBufferToImage2)
        }
        b"vkDestroyDevice" => ours!(destroy_device as vk::PFN_vkDestroyDevice),
        _ => None,
    }
}

unsafe extern "system" fn get_features(physical: vk::PhysicalDevice, features: *mut vk::PhysicalDeviceFeatures) {
    let Some(i) = instance_table() else { return };
    // SAFETY: the engine's arguments.
    unsafe {
        (i.fp_v1_0().get_physical_device_features)(physical, features);
        if let Some(f) = features.as_mut() {
            f.texture_compression_etc2 = vk::TRUE;
        }
    }
}

unsafe extern "system" fn get_features2(physical: vk::PhysicalDevice, features: *mut vk::PhysicalDeviceFeatures2<'_>) {
    let Some(i) = instance_table() else { return };
    // SAFETY: the engine's arguments.
    unsafe {
        (i.fp_v1_1().get_physical_device_features2)(physical, features);
        if let Some(f) = features.as_mut() {
            f.features.texture_compression_etc2 = vk::TRUE;
        }
    }
}

fn masked(props: &mut vk::FormatProperties) {
    props.optimal_tiling_features &= KEEP;
    props.linear_tiling_features = vk::FormatFeatureFlags::empty();
    props.buffer_features = vk::FormatFeatureFlags::empty();
}

unsafe extern "system" fn get_format_properties(
    physical: vk::PhysicalDevice,
    format: vk::Format,
    props: *mut vk::FormatProperties,
) {
    let Some(i) = instance_table() else { return };
    let plain = substitute(format);
    // SAFETY: the engine's arguments (with the decoded format).
    unsafe {
        (i.fp_v1_0().get_physical_device_format_properties)(physical, plain.map_or(format, |p| p.1), props);
        if plain.is_some()
            && let Some(p) = props.as_mut()
        {
            masked(p);
        }
    }
}

unsafe extern "system" fn get_format_properties2(
    physical: vk::PhysicalDevice,
    format: vk::Format,
    props: *mut vk::FormatProperties2<'_>,
) {
    let Some(i) = instance_table() else { return };
    let plain = substitute(format);
    // SAFETY: the engine's arguments (with the decoded format).
    unsafe {
        (i.fp_v1_1().get_physical_device_format_properties2)(physical, plain.map_or(format, |p| p.1), props);
        if plain.is_some()
            && let Some(p) = props.as_mut()
        {
            masked(&mut p.format_properties);
        }
    }
}

/// Image flags that only make sense for compressed formats.
fn plain_flags(flags: vk::ImageCreateFlags) -> vk::ImageCreateFlags {
    flags & !vk::ImageCreateFlags::BLOCK_TEXEL_VIEW_COMPATIBLE
}

unsafe extern "system" fn get_image_format_properties(
    physical: vk::PhysicalDevice,
    format: vk::Format,
    kind: vk::ImageType,
    tiling: vk::ImageTiling,
    usage: vk::ImageUsageFlags,
    flags: vk::ImageCreateFlags,
    props: *mut vk::ImageFormatProperties,
) -> vk::Result {
    let Some(i) = instance_table() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let (format, flags) = match substitute(format) {
        Some((_, plain)) if tiling == vk::ImageTiling::OPTIMAL => (plain, plain_flags(flags)),
        Some(_) => return vk::Result::ERROR_FORMAT_NOT_SUPPORTED,
        None => (format, flags),
    };
    // SAFETY: the engine's arguments (with the decoded format).
    unsafe { (i.fp_v1_0().get_physical_device_image_format_properties)(physical, format, kind, tiling, usage, flags, props) }
}

unsafe extern "system" fn get_image_format_properties2(
    physical: vk::PhysicalDevice,
    info: *const vk::PhysicalDeviceImageFormatInfo2<'_>,
    props: *mut vk::ImageFormatProperties2<'_>,
) -> vk::Result {
    let Some(i) = instance_table() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the engine's valid info.
    let mut copy = unsafe { *info };
    if let Some((_, plain)) = substitute(copy.format) {
        if copy.tiling != vk::ImageTiling::OPTIMAL {
            return vk::Result::ERROR_FORMAT_NOT_SUPPORTED;
        }
        copy.format = plain;
        copy.flags = plain_flags(copy.flags);
    }
    // SAFETY: the engine's arguments (with the decoded format).
    unsafe { (i.fp_v1_1().get_physical_device_image_format_properties2)(physical, &copy, props) }
}

/// The current device's tables, if `device` is it.
fn table(device: vk::Device) -> Option<ash::Device> {
    with_dev(|d| (d.handle == device.as_raw()).then(|| d.table.clone())).flatten()
}

/// The host's device function `name` (for a device Feline doesn't track).
fn host_device_fn<T>(device: vk::Device, name: &CStr) -> Option<T> {
    let f = crate::vulkan::host_device_function(device.as_raw() as *mut c_void, name)?;
    // SAFETY: the caller names the function's real type.
    Some(unsafe { std::mem::transmute_copy::<unsafe extern "C" fn(), T>(&f) })
}

unsafe extern "system" fn create_image(
    device: vk::Device,
    info: *const vk::ImageCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    image: *mut vk::Image,
) -> vk::Result {
    let Some(t) = table(device) else {
        let Some(f) = host_device_fn::<vk::PFN_vkCreateImage>(device, c"vkCreateImage") else {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        };
        // SAFETY: the engine's arguments.
        return unsafe { f(device, info, allocator, image) };
    };
    // SAFETY: the engine's valid info.
    let mut copy = unsafe { *info };
    let etc = substitute(copy.format);
    if let Some((_, plain)) = etc {
        copy.format = plain;
        copy.flags = plain_flags(copy.flags);
    }
    // SAFETY: the engine's arguments (with the decoded format).
    let result = unsafe { (t.fp_v1_0().create_image)(device, &copy, allocator, image) };
    if result == vk::Result::SUCCESS
        && let Some((kind, _)) = etc
    {
        // SAFETY: filled by the driver.
        let made = unsafe { *image }.as_raw();
        with_dev(|d| d.images.insert(made, kind));
    }
    result
}

unsafe extern "system" fn destroy_image(device: vk::Device, image: vk::Image, allocator: *const vk::AllocationCallbacks<'_>) {
    with_dev(|d| d.images.remove(&image.as_raw()));
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_0().destroy_image),
        None => host_device_fn::<vk::PFN_vkDestroyImage>(device, c"vkDestroyImage"),
    };
    if let Some(f) = f {
        // SAFETY: the engine's arguments.
        unsafe { f(device, image, allocator) };
    }
}

unsafe extern "system" fn create_image_view(
    device: vk::Device,
    info: *const vk::ImageViewCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    view: *mut vk::ImageView,
) -> vk::Result {
    // SAFETY: the engine's valid info.
    let mut copy = unsafe { *info };
    if let Some((_, plain)) = substitute(copy.format) {
        copy.format = plain;
    }
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_0().create_image_view),
        None => host_device_fn::<vk::PFN_vkCreateImageView>(device, c"vkCreateImageView"),
    };
    let Some(f) = f else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the engine's arguments (with the decoded format).
    unsafe { f(device, &copy, allocator, view) }
}

unsafe extern "system" fn bind_buffer_memory(
    device: vk::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    offset: vk::DeviceSize,
) -> vk::Result {
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_0().bind_buffer_memory),
        None => host_device_fn::<vk::PFN_vkBindBufferMemory>(device, c"vkBindBufferMemory"),
    };
    let Some(f) = f else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the engine's arguments.
    let result = unsafe { f(device, buffer, memory, offset) };
    if result == vk::Result::SUCCESS {
        with_dev(|d| d.buffers.insert(buffer.as_raw(), (memory.as_raw(), offset)));
    }
    result
}

unsafe extern "system" fn bind_buffer_memory2(
    device: vk::Device,
    count: u32,
    infos: *const vk::BindBufferMemoryInfo<'_>,
) -> vk::Result {
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_1().bind_buffer_memory2),
        None => host_device_fn::<vk::PFN_vkBindBufferMemory2>(device, c"vkBindBufferMemory2"),
    };
    let Some(f) = f else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the engine's arguments.
    let result = unsafe { f(device, count, infos) };
    if result == vk::Result::SUCCESS && !infos.is_null() {
        // SAFETY: `count` valid infos.
        let infos = unsafe { std::slice::from_raw_parts(infos, count as usize) };
        with_dev(|d| {
            for i in infos {
                d.buffers.insert(i.buffer.as_raw(), (i.memory.as_raw(), i.memory_offset));
            }
        });
    }
    result
}

unsafe extern "system" fn destroy_buffer(device: vk::Device, buffer: vk::Buffer, allocator: *const vk::AllocationCallbacks<'_>) {
    with_dev(|d| d.buffers.remove(&buffer.as_raw()));
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_0().destroy_buffer),
        None => host_device_fn::<vk::PFN_vkDestroyBuffer>(device, c"vkDestroyBuffer"),
    };
    if let Some(f) = f {
        // SAFETY: the engine's arguments.
        unsafe { f(device, buffer, allocator) };
    }
}

unsafe extern "system" fn map_memory(
    device: vk::Device,
    memory: vk::DeviceMemory,
    offset: vk::DeviceSize,
    size: vk::DeviceSize,
    flags: vk::MemoryMapFlags,
    data: *mut *mut c_void,
) -> vk::Result {
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_0().map_memory),
        None => host_device_fn::<vk::PFN_vkMapMemory>(device, c"vkMapMemory"),
    };
    let Some(f) = f else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the engine's arguments.
    let result = unsafe { f(device, memory, offset, size, flags, data) };
    if result == vk::Result::SUCCESS && !data.is_null() {
        // SAFETY: filled by the driver.
        let address = unsafe { *data } as usize;
        with_dev(|d| d.maps.insert(memory.as_raw(), (offset, address)));
    }
    result
}

unsafe extern "system" fn unmap_memory(device: vk::Device, memory: vk::DeviceMemory) {
    with_dev(|d| d.maps.remove(&memory.as_raw()));
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_0().unmap_memory),
        None => host_device_fn::<vk::PFN_vkUnmapMemory>(device, c"vkUnmapMemory"),
    };
    if let Some(f) = f {
        // SAFETY: the engine's arguments.
        unsafe { f(device, memory) };
    }
}

unsafe extern "system" fn free_memory(device: vk::Device, memory: vk::DeviceMemory, allocator: *const vk::AllocationCallbacks<'_>) {
    with_dev(|d| d.maps.remove(&memory.as_raw()));
    let f = match table(device) {
        Some(t) => Some(t.fp_v1_0().free_memory),
        None => host_device_fn::<vk::PFN_vkFreeMemory>(device, c"vkFreeMemory"),
    };
    if let Some(f) = f {
        // SAFETY: the engine's arguments.
        unsafe { f(device, memory, allocator) };
    }
}

unsafe extern "system" fn destroy_device(device: vk::Device, allocator: *const vk::AllocationCallbacks<'_>) {
    // Feline's staging memory goes first (the engine waited for the device
    // to be idle before destroying it).
    let mut guard = STATE.lock().unwrap_or_else(|p| p.into_inner());
    if guard.as_ref().is_some_and(|d| d.handle == device.as_raw())
        && let Some(dev) = guard.take()
    {
        for chunk in &dev.staging {
            // SAFETY: Feline's own objects on the idle device.
            unsafe {
                dev.table.unmap_memory(chunk.memory);
                dev.table.destroy_buffer(chunk.buffer, None);
                dev.table.free_memory(chunk.memory, None);
            }
        }
    }
    drop(guard);
    if let Some(f) = host_device_fn::<vk::PFN_vkDestroyDevice>(device, c"vkDestroyDevice") {
        // SAFETY: the engine's arguments.
        unsafe { f(device, allocator) };
    }
}

// ---------------------------------------------------------------------------
// Uploads
// ---------------------------------------------------------------------------

/// A piece of Feline's staging memory.
struct Chunk {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    address: usize,
    size: u64,
    used: u64,
    /// When it was last written to (frame, time); reused once both are well past.
    last: (u64, Instant),
}

const CHUNK: u64 = 32 << 20;
/// Frames and time after which the GPU has surely read a chunk.
const SAFE_FRAMES: u64 = 16;
const SAFE_TIME: Duration = Duration::from_secs(2);

impl Dev {
    /// Room for `bytes` in staging memory: (buffer, offset, address).
    fn stage(&mut self, bytes: u64) -> Option<(vk::Buffer, u64, usize)> {
        let frame = FRAMES.load(Ordering::Relaxed);
        let now = Instant::now();
        if let Some(i) = self.active {
            let c = &mut self.staging[i];
            let at = c.used.next_multiple_of(16);
            if at + bytes <= c.size {
                c.used = at + bytes;
                c.last = (frame, now);
                return Some((c.buffer, at, c.address + at as usize));
            }
        }
        // A chunk the GPU is done with, or a new one.
        let free = self.staging.iter().position(|c| {
            c.size >= bytes && frame >= c.last.0 + SAFE_FRAMES && now.duration_since(c.last.1) >= SAFE_TIME
        });
        let i = match free {
            Some(i) => i,
            None => {
                let chunk = self.new_chunk(bytes.max(CHUNK))?;
                self.staging.push(chunk);
                self.staging.len() - 1
            }
        };
        self.active = Some(i);
        let c = &mut self.staging[i];
        c.used = bytes;
        c.last = (frame, now);
        Some((c.buffer, 0, c.address))
    }

    fn new_chunk(&self, size: u64) -> Option<Chunk> {
        let t = &self.table;
        // SAFETY: Vulkan calls on the engine's live device, with valid info.
        unsafe {
            let buffer = t
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size)
                        .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                    None,
                )
                .ok()?;
            let needs = t.get_buffer_memory_requirements(buffer);
            let wanted = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let props = &self.memory_props;
            let Some(kind) = (0..props.memory_type_count).find(|&i| {
                needs.memory_type_bits & (1 << i) != 0 && props.memory_types[i as usize].property_flags.contains(wanted)
            }) else {
                t.destroy_buffer(buffer, None);
                return None;
            };
            let Ok(memory) = t.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            ) else {
                t.destroy_buffer(buffer, None);
                return None;
            };
            let mapped = t
                .bind_buffer_memory(buffer, memory, 0)
                .and_then(|()| t.map_memory(memory, 0, size, vk::MemoryMapFlags::empty()));
            let Ok(address) = mapped else {
                t.destroy_buffer(buffer, None);
                t.free_memory(memory, None);
                return None;
            };
            eprintln!(
                "feline-runtime: vulkan: {} MB of staging memory for ETC2 textures",
                size >> 20
            );
            Some(Chunk {
                buffer,
                memory,
                address: address as usize,
                size,
                used: 0,
                last: (0, Instant::now()),
            })
        }
    }

    /// Where buffer `buffer`'s byte `offset` is in Feline's address space.
    fn source(&self, buffer: vk::Buffer, offset: u64) -> Option<*const u8> {
        let (memory, bound) = *self.buffers.get(&buffer.as_raw())?;
        let (mapped_from, address) = *self.maps.get(&memory)?;
        let at = (bound + offset).checked_sub(mapped_from)?;
        Some((address + at as usize) as *const u8)
    }

    /// One region of an upload into an ETC2 image, decoded into staging
    /// memory: the region to copy from Feline's buffer instead.
    fn decode_region(
        &mut self,
        kind: Kind,
        src: vk::Buffer,
        region: &vk::BufferImageCopy,
    ) -> Option<(vk::Buffer, vk::BufferImageCopy)> {
        let e = region.image_extent;
        let (w, h) = (e.width as usize, e.height as usize);
        let layers = match region.image_subresource.layer_count {
            vk::REMAINING_ARRAY_LAYERS => 1,
            n => n as usize,
        };
        let slices = layers * e.depth.max(1) as usize;
        let row_texels = if region.buffer_row_length == 0 { w } else { region.buffer_row_length as usize };
        let height_texels = if region.buffer_image_height == 0 { h } else { region.buffer_image_height as usize };
        let row_blocks = row_texels.div_ceil(4);
        let slice_bytes = row_blocks * height_texels.div_ceil(4) * kind.block_bytes();
        let Some(from) = self.source(src, region.buffer_offset) else {
            if !self.told_unmapped {
                self.told_unmapped = true;
                eprintln!("feline-runtime: vulkan: an ETC2 upload from memory that isn't mapped; skipped");
            }
            return None;
        };
        let out_slice = w * h * kind.pixel_bytes();
        let (buffer, offset, address) = self.stage((out_slice * slices) as u64)?;
        // SAFETY: the engine's mapped staging memory holds the upload's
        // blocks (it wrote them before recording the copy), and Feline's
        // chunk has room for the decoded pixels.
        let (input, output) = unsafe {
            (
                std::slice::from_raw_parts(from, slice_bytes * slices),
                std::slice::from_raw_parts_mut(address as *mut u8, out_slice * slices),
            )
        };
        for s in 0..slices {
            crate::etc2::decode_area(
                kind,
                &input[s * slice_bytes..],
                row_blocks,
                w,
                h,
                &mut output[s * out_slice..(s + 1) * out_slice],
            );
        }
        let mut changed = *region;
        changed.buffer_offset = offset;
        changed.buffer_row_length = 0;
        changed.buffer_image_height = 0;
        Some((buffer, changed))
    }
}

/// Logs the first ETC2 upload (to show it works).
fn told_first_upload() {
    static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !TOLD.swap(true, Ordering::Relaxed) {
        eprintln!("feline-runtime: vulkan: first ETC2 texture decoded");
    }
}

unsafe extern "system" fn cmd_copy_buffer_to_image(
    commands: vk::CommandBuffer,
    src: vk::Buffer,
    image: vk::Image,
    layout: vk::ImageLayout,
    count: u32,
    regions: *const vk::BufferImageCopy,
) {
    let mut guard = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let Some(dev) = guard.as_mut() else { return };
    let copy = dev.table.fp_v1_0().cmd_copy_buffer_to_image;
    let Some(&kind) = dev.images.get(&image.as_raw()) else {
        drop(guard);
        // SAFETY: the engine's arguments.
        unsafe { copy(commands, src, image, layout, count, regions) };
        return;
    };
    // SAFETY: `count` valid regions.
    let regions = unsafe { std::slice::from_raw_parts(regions, count as usize) };
    let decoded: Vec<_> = regions.iter().filter_map(|r| dev.decode_region(kind, src, r)).collect();
    drop(guard);
    if !decoded.is_empty() {
        told_first_upload();
    }
    for (buffer, region) in decoded {
        // SAFETY: Feline's buffer and the engine's image and command buffer.
        unsafe { copy(commands, buffer, image, layout, 1, &region) };
    }
}

unsafe extern "system" fn cmd_copy_buffer_to_image2(commands: vk::CommandBuffer, info: *const vk::CopyBufferToImageInfo2<'_>) {
    let mut guard = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let Some(dev) = guard.as_mut() else { return };
    let copy2 = dev.table.fp_v1_3().cmd_copy_buffer_to_image2;
    let copy = dev.table.fp_v1_0().cmd_copy_buffer_to_image;
    // SAFETY: the engine's valid info.
    let i = unsafe { &*info };
    let Some(&kind) = dev.images.get(&i.dst_image.as_raw()) else {
        drop(guard);
        // SAFETY: the engine's arguments.
        unsafe { copy2(commands, info) };
        return;
    };
    // SAFETY: `region_count` valid regions.
    let regions = unsafe { std::slice::from_raw_parts(i.p_regions, i.region_count as usize) };
    let decoded: Vec<_> = regions
        .iter()
        .filter_map(|r| {
            let plain = vk::BufferImageCopy {
                buffer_offset: r.buffer_offset,
                buffer_row_length: r.buffer_row_length,
                buffer_image_height: r.buffer_image_height,
                image_subresource: r.image_subresource,
                image_offset: r.image_offset,
                image_extent: r.image_extent,
            };
            dev.decode_region(kind, i.src_buffer, &plain)
        })
        .collect();
    drop(guard);
    if !decoded.is_empty() {
        told_first_upload();
    }
    for (buffer, region) in decoded {
        // SAFETY: Feline's buffer and the engine's image and command buffer.
        unsafe { copy(commands, buffer, i.dst_image, i.dst_image_layout, 1, &region) };
    }
}
