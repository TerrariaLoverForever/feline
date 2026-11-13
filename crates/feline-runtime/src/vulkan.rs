// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Vulkan for the engine (`libvulkan.so`), on the desktop's Vulkan driver.
//!
//! Android's x86_64 Vulkan and the desktop's share the calling convention
//! and every struct layout, so almost every function is the host's own.
//! Only what's Android-specific is translated:
//!
//! - the window: `VK_KHR_android_surface` (an `ANativeWindow*`) is offered
//!   to the engine, and created as a Wayland (or X11) surface on Feline's
//!   window;
//! - extensions the engine enables that the driver doesn't have are left
//!   out (Android-only ones);
//! - the window size: Wayland leaves a surface's current size undefined,
//!   which Android apps don't expect, so it's filled in;
//! - presenting: the swapchain doesn't wait for the monitor (MAILBOX, or
//!   IMMEDIATE), so Roblox's frame rate limit decides, as with OpenGL.
//!
//! On whenever the desktop has a Vulkan driver (it's much faster than
//! OpenGL ES here), unless the `renderer` setting is "opengl". Without a
//! driver, with that setting, or with `FELINE_VULKAN=0` (for
//! troubleshooting), `libvulkan.so` is unavailable and the engine uses
//! OpenGL ES. `FELINE_VULKAN=1` offers Vulkan whatever the setting.
//!
//! Unsafe: Vulkan's C API, called with the engine's own arguments.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

type Pfn = Option<unsafe extern "C" fn()>;
type GetInstanceProcAddr = unsafe extern "C" fn(*mut c_void, *const c_char) -> Pfn;

const VK_SUCCESS: i32 = 0;
const VK_INCOMPLETE: i32 = 5;
const ANDROID_SURFACE: &CStr = c"VK_KHR_android_surface";
const WAYLAND_SURFACE: &CStr = c"VK_KHR_wayland_surface";
const XLIB_SURFACE: &CStr = c"VK_KHR_xlib_surface";
const XCB_SURFACE: &CStr = c"VK_KHR_xcb_surface";
const STYPE_XCB_SURFACE: u32 = 1_000_005_000;
const STYPE_WAYLAND_SURFACE: u32 = 1_000_006_000;
const STYPE_XLIB_SURFACE: u32 = 1_000_004_000;
const PRESENT_MODE_IMMEDIATE: u32 = 0;
const PRESENT_MODE_MAILBOX: u32 = 1;

/// Whether Vulkan is offered to the engine.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let wanted = match std::env::var("FELINE_VULKAN").as_deref() {
            Ok("0") => false,
            Ok("1") => true,
            _ => crate::prefs::get().renderer != "opengl",
        };
        if !wanted {
            eprintln!("feline-runtime: OpenGL ES chosen: Vulkan isn't offered to the engine");
        }
        wanted && host().is_some()
    })
}

struct Host {
    get_instance_proc_addr: GetInstanceProcAddr,
}

fn host() -> Option<&'static Host> {
    static HOST: OnceLock<Option<Host>> = OnceLock::new();
    HOST.get_or_init(|| {
        // SAFETY: a valid C string; the library is never closed.
        let handle = unsafe { libc::dlopen(c"libvulkan.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            eprintln!("feline-runtime: vulkan: libvulkan.so.1 isn't installed");
            return None;
        }
        // SAFETY: a live handle; Vulkan's documented loader entry point.
        let gipa = unsafe { libc::dlsym(handle, c"vkGetInstanceProcAddr".as_ptr()) };
        if gipa.is_null() {
            return None;
        }
        Some(Host {
            // SAFETY: vkGetInstanceProcAddr's signature.
            get_instance_proc_addr: unsafe { std::mem::transmute::<*mut c_void, GetInstanceProcAddr>(gipa) },
        })
    })
    .as_ref()
}

/// The driver's own `vkGetInstanceProcAddr` (for `vkoverlay`'s tables).
pub fn host_get_instance_proc_addr() -> Option<usize> {
    Some(host()?.get_instance_proc_addr as usize)
}

/// The host's function `name` for `instance` (NULL instance: global ones).
fn host_proc(instance: *mut c_void, name: &CStr) -> Pfn {
    let host = host()?;
    // SAFETY: a valid C string and an instance from the host (or NULL).
    unsafe { (host.get_instance_proc_addr)(instance, name.as_ptr()) }
}

/// `dlsym(libvulkan.so, name)`: Feline's version for the translated
/// functions, the host's for the rest.
pub fn symbol(name: &str) -> Option<usize> {
    let name = CString::new(name).ok()?;
    // SAFETY: global lookup; the result is a plain function address.
    let proc = unsafe { get_instance_proc_addr(std::ptr::null_mut(), name.as_ptr()) };
    proc.map(|f| f as usize)
}

macro_rules! ours {
    ($f:expr) => {
        Some(unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn()>($f as *const ()) })
    };
}

/// # Safety
/// Vulkan's contract: `name` is a valid C string.
pub unsafe extern "C" fn get_instance_proc_addr(instance: *mut c_void, name: *const c_char) -> Pfn {
    // SAFETY: the caller passes a valid C string.
    let wanted = unsafe { CStr::from_ptr(name) };
    match wanted.to_bytes() {
        b"vkGetInstanceProcAddr" => ours!(get_instance_proc_addr as unsafe extern "C" fn(*mut c_void, *const c_char) -> Pfn),
        b"vkEnumerateInstanceExtensionProperties" => ours!(enumerate_instance_extensions as EnumerateExtensions),
        b"vkCreateInstance" => ours!(create_instance as CreateInstance),
        b"vkCreateAndroidSurfaceKHR" => ours!(create_android_surface as CreateAndroidSurface),
        b"vkCreateDevice" => ours!(create_device as CreateDevice),
        b"vkGetPhysicalDeviceSurfaceCapabilitiesKHR" => ours!(surface_capabilities as SurfaceCapabilities),
        b"vkCreateSwapchainKHR" => ours!(create_swapchain as CreateSwapchain),
        b"vkGetDeviceProcAddr" => ours!(get_device_proc_addr as GetDeviceProcAddr),
        b"vkGetDeviceQueue" => ours!(get_device_queue as GetDeviceQueue),
        b"vkGetDeviceQueue2" => ours!(get_device_queue2 as GetDeviceQueue2),
        b"vkQueuePresentKHR" => {
            if let Some(f) = host_proc(instance, wanted) {
                HOST_PRESENT.store(f as usize, Ordering::Release);
                ours!(queue_present as QueuePresent)
            } else {
                None
            }
        }
        _ => host_proc(instance, wanted),
    }
}

type GetDeviceProcAddr = unsafe extern "C" fn(*mut c_void, *const c_char) -> Pfn;

/// Device functions: the host's, except the swapchain's creation.
///
/// # Safety
/// Vulkan's contract: `name` is a valid C string.
unsafe extern "C" fn get_device_proc_addr(device: *mut c_void, name: *const c_char) -> Pfn {
    // SAFETY: the caller passes a valid C string.
    let wanted = unsafe { CStr::from_ptr(name) };
    if wanted.to_bytes() == b"vkCreateSwapchainKHR" {
        return ours!(create_swapchain as CreateSwapchain);
    }
    match wanted.to_bytes() {
        b"vkGetDeviceQueue" => return ours!(get_device_queue as GetDeviceQueue),
        b"vkGetDeviceQueue2" => return ours!(get_device_queue2 as GetDeviceQueue2),
        _ => {}
    }
    if wanted.to_bytes() == b"vkQueuePresentKHR" {
        let instance = instance();
        if let Some(host_gdpa) = host_proc(instance, c"vkGetDeviceProcAddr") {
            // SAFETY: the host's vkGetDeviceProcAddr.
            let host_gdpa: GetDeviceProcAddr = unsafe { std::mem::transmute(host_gdpa) };
            // SAFETY: the engine's device and name.
            if let Some(f) = unsafe { host_gdpa(device, name) } {
                HOST_PRESENT.store(f as usize, Ordering::Release);
                return ours!(queue_present as QueuePresent);
            }
        }
    }
    let instance = instance();
    let host_gdpa = host_proc(instance, c"vkGetDeviceProcAddr")?;
    // SAFETY: the host's vkGetDeviceProcAddr.
    let host_gdpa: GetDeviceProcAddr = unsafe { std::mem::transmute(host_gdpa) };
    // SAFETY: the engine's device and name.
    unsafe { host_gdpa(device, name) }
}

#[repr(C)]
struct ExtensionProperties {
    name: [c_char; 256],
    spec_version: u32,
}

type EnumerateExtensions = unsafe extern "C" fn(*const c_char, *mut u32, *mut ExtensionProperties) -> i32;

/// The host's instance extensions plus `VK_KHR_android_surface` (when the
/// host can make a surface on Feline's window).
unsafe extern "C" fn enumerate_instance_extensions(
    layer: *const c_char,
    count: *mut u32,
    properties: *mut ExtensionProperties,
) -> i32 {
    let Some(host_enum) = host_proc(std::ptr::null_mut(), c"vkEnumerateInstanceExtensionProperties") else {
        return -7; // VK_ERROR_EXTENSION_NOT_PRESENT
    };
    // SAFETY: the host's function with that signature.
    let host_enum: EnumerateExtensions = unsafe { std::mem::transmute(host_enum) };
    if !layer.is_null() {
        // SAFETY: passed through.
        return unsafe { host_enum(layer, count, properties) };
    }
    let mut n = 0u32;
    // SAFETY: counting call.
    unsafe { host_enum(layer, &mut n, std::ptr::null_mut()) };
    // SAFETY: an all-zero ExtensionProperties is valid.
    let mut list: Vec<ExtensionProperties> = (0..n).map(|_| unsafe { std::mem::zeroed() }).collect();
    // SAFETY: room for `n`.
    unsafe { host_enum(layer, &mut n, list.as_mut_ptr()) };
    list.truncate(n as usize);
    // SAFETY: as above.
    let mut android: ExtensionProperties = unsafe { std::mem::zeroed() };
    for (i, b) in ANDROID_SURFACE.to_bytes().iter().enumerate() {
        android.name[i] = *b as c_char;
    }
    android.spec_version = 6;
    list.push(android);
    // SAFETY: the caller's count pointer and (optional) array of `*count`.
    unsafe {
        if properties.is_null() {
            *count = list.len() as u32;
            return VK_SUCCESS;
        }
        let room = *count as usize;
        let copied = room.min(list.len());
        std::ptr::copy_nonoverlapping(list.as_ptr(), properties, copied);
        *count = copied as u32;
        if copied < list.len() { VK_INCOMPLETE } else { VK_SUCCESS }
    }
}

/// `VkInstanceCreateInfo` / `VkDeviceCreateInfo`: the parts used here.
#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    application_info: *const c_void,
    layer_count: u32,
    layers: *const *const c_char,
    extension_count: u32,
    extensions: *const *const c_char,
}

#[repr(C)]
struct DeviceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_create_info_count: u32,
    queue_create_infos: *const c_void,
    layer_count: u32,
    layers: *const *const c_char,
    extension_count: u32,
    extensions: *const *const c_char,
    enabled_features: *const c_void,
}

type CreateInstance = unsafe extern "C" fn(*const InstanceCreateInfo, *const c_void, *mut *mut c_void) -> i32;

/// Which desktop surface extension the window needs.
fn window_surface_extension() -> &'static CStr {
    match crate::window::native_window().map(|w| w.handles) {
        Some(crate::window::NativeHandles::Xlib { .. }) => XLIB_SURFACE,
        Some(crate::window::NativeHandles::Xcb { .. }) => XCB_SURFACE,
        _ => WAYLAND_SURFACE,
    }
}

unsafe extern "C" fn create_instance(
    info: *const InstanceCreateInfo,
    allocator: *const c_void,
    instance: *mut *mut c_void,
) -> i32 {
    let Some(host_create) = host_proc(std::ptr::null_mut(), c"vkCreateInstance") else {
        return -3; // VK_ERROR_INITIALIZATION_FAILED
    };
    // SAFETY: the host's function with that signature.
    let host_create: CreateInstance = unsafe { std::mem::transmute(host_create) };
    // SAFETY: the engine's valid create info.
    let mut copy = unsafe { std::ptr::read(info) };
    let names = unsafe { names(copy.extensions, copy.extension_count) };
    let mut wanted: Vec<*const c_char> = Vec::new();
    for name in &names {
        if *name == ANDROID_SURFACE {
            wanted.push(window_surface_extension().as_ptr());
        } else {
            wanted.push(name.as_ptr());
        }
    }
    eprintln!(
        "feline-runtime: vulkan: instance with {:?} (Android surface → {:?})",
        names.iter().map(|n| n.to_string_lossy()).collect::<Vec<_>>(),
        window_surface_extension()
    );
    copy.extensions = wanted.as_ptr();
    copy.extension_count = wanted.len() as u32;
    // SAFETY: the engine's create info with a valid extension list.
    let result = unsafe { host_create(&copy, allocator, instance) };
    if result == VK_SUCCESS {
        // SAFETY: filled by the driver.
        INSTANCE.store(unsafe { *instance } as usize, Ordering::Release);
    } else {
        eprintln!("feline-runtime: vulkan: vkCreateInstance failed ({result})");
    }
    result
}

/// The C strings of an extension list.
///
/// # Safety
/// `list` holds `count` valid C strings (or is NULL with 0).
unsafe fn names<'a>(list: *const *const c_char, count: u32) -> Vec<&'a CStr> {
    (0..count as usize)
        // SAFETY: as documented.
        .map(|i| unsafe { CStr::from_ptr(*list.add(i)) })
        .collect()
}

#[repr(C)]
struct AndroidSurfaceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    window: *mut c_void,
}

#[repr(C)]
struct WaylandSurfaceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    display: *mut c_void,
    surface: *mut c_void,
}

#[repr(C)]
struct XlibSurfaceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    display: *mut c_void,
    window: u64,
}

type CreateAndroidSurface =
    unsafe extern "C" fn(*mut c_void, *const AndroidSurfaceCreateInfo, *const c_void, *mut u64) -> i32;

/// The engine's Android surface, made on Feline's desktop window.
unsafe extern "C" fn create_android_surface(
    instance: *mut c_void,
    _info: *const AndroidSurfaceCreateInfo,
    allocator: *const c_void,
    surface: *mut u64,
) -> i32 {
    type CreateWayland = unsafe extern "C" fn(*mut c_void, *const WaylandSurfaceCreateInfo, *const c_void, *mut u64) -> i32;
    type CreateXlib = unsafe extern "C" fn(*mut c_void, *const XlibSurfaceCreateInfo, *const c_void, *mut u64) -> i32;
    let result = match crate::window::native_window().map(|w| w.handles) {
        Some(crate::window::NativeHandles::Wayland { display, surface: wl_surface }) => {
            let Some(f) = host_proc(instance, c"vkCreateWaylandSurfaceKHR") else {
                return -7;
            };
            // SAFETY: the host's function with that signature.
            let f: CreateWayland = unsafe { std::mem::transmute(f) };
            let info = WaylandSurfaceCreateInfo {
                s_type: STYPE_WAYLAND_SURFACE,
                p_next: std::ptr::null(),
                flags: 0,
                display: display as *mut c_void,
                surface: wl_surface as *mut c_void,
            };
            // SAFETY: Feline's live Wayland display and surface.
            unsafe { f(instance, &info, allocator, surface) }
        }
        Some(crate::window::NativeHandles::Xlib { display, window }) => {
            let Some(f) = host_proc(instance, c"vkCreateXlibSurfaceKHR") else {
                return -7;
            };
            // SAFETY: the host's function with that signature.
            let f: CreateXlib = unsafe { std::mem::transmute(f) };
            let info = XlibSurfaceCreateInfo {
                s_type: STYPE_XLIB_SURFACE,
                p_next: std::ptr::null(),
                flags: 0,
                display: display as *mut c_void,
                window,
            };
            // SAFETY: Feline's live X11 display and window.
            unsafe { f(instance, &info, allocator, surface) }
        }
        Some(crate::window::NativeHandles::Xcb { connection, window }) => {
            #[repr(C)]
            struct XcbSurfaceCreateInfo {
                s_type: u32,
                p_next: *const c_void,
                flags: u32,
                connection: *mut c_void,
                window: u32,
            }
            type CreateXcb = unsafe extern "C" fn(*mut c_void, *const XcbSurfaceCreateInfo, *const c_void, *mut u64) -> i32;
            let Some(f) = host_proc(instance, c"vkCreateXcbSurfaceKHR") else {
                return -7;
            };
            // SAFETY: the host's function with that signature.
            let f: CreateXcb = unsafe { std::mem::transmute(f) };
            let info = XcbSurfaceCreateInfo {
                s_type: STYPE_XCB_SURFACE,
                p_next: std::ptr::null(),
                flags: 0,
                connection: connection as *mut c_void,
                window,
            };
            // SAFETY: Feline's live XCB connection and window.
            unsafe { f(instance, &info, allocator, surface) }
        }
        None => -3,
    };
    eprintln!("feline-runtime: vulkan: window surface created ({result})");
    result
}

type CreateDevice = unsafe extern "C" fn(*mut c_void, *const DeviceCreateInfo, *const c_void, *mut *mut c_void) -> i32;
type EnumerateDeviceExtensions = unsafe extern "C" fn(*mut c_void, *const c_char, *mut u32, *mut ExtensionProperties) -> i32;

/// The device, without the extensions the driver doesn't have (Android's).
unsafe extern "C" fn create_device(
    physical: *mut c_void,
    info: *const DeviceCreateInfo,
    allocator: *const c_void,
    device: *mut *mut c_void,
) -> i32 {
    let (Some(host_create), Some(host_enum)) = (
        host_proc(instance(), c"vkCreateDevice"),
        host_proc(instance(), c"vkEnumerateDeviceExtensionProperties"),
    ) else {
        return -3;
    };
    // SAFETY: the host's functions with those signatures.
    let host_create: CreateDevice = unsafe { std::mem::transmute(host_create) };
    let host_enum: EnumerateDeviceExtensions = unsafe { std::mem::transmute(host_enum) };
    let mut n = 0u32;
    // SAFETY: counting call on the engine's physical device.
    unsafe { host_enum(physical, std::ptr::null(), &mut n, std::ptr::null_mut()) };
    // SAFETY: an all-zero ExtensionProperties is valid.
    let mut available: Vec<ExtensionProperties> = (0..n).map(|_| unsafe { std::mem::zeroed() }).collect();
    // SAFETY: room for `n`.
    unsafe { host_enum(physical, std::ptr::null(), &mut n, available.as_mut_ptr()) };
    available.truncate(n as usize);
    let has = |name: &CStr| {
        available
            .iter()
            // SAFETY: the driver's NUL-terminated names.
            .any(|p| unsafe { CStr::from_ptr(p.name.as_ptr()) } == name)
    };
    // SAFETY: the engine's valid create info.
    let mut copy = unsafe { std::ptr::read(info) };
    let names = unsafe { names(copy.extensions, copy.extension_count) };
    let (kept, dropped): (Vec<&CStr>, Vec<&CStr>) = names.iter().partition(|n| has(n));
    eprintln!(
        "feline-runtime: vulkan: device with {} extensions; not on this driver: {:?}",
        kept.len(),
        dropped.iter().map(|n| n.to_string_lossy()).collect::<Vec<_>>()
    );
    let pointers: Vec<*const c_char> = kept.iter().map(|n| n.as_ptr()).collect();
    copy.extensions = pointers.as_ptr();
    copy.extension_count = pointers.len() as u32;
    PHYSICAL.store(physical as usize, Ordering::Release);
    // SAFETY: the engine's create info with a valid extension list.
    let result = unsafe { host_create(physical, &copy, allocator, device) };
    if result == VK_SUCCESS {
        // SAFETY: filled by the driver.
        crate::vkoverlay::device_made(unsafe { *device } as u64, physical as u64);
    } else {
        eprintln!("feline-runtime: vulkan: vkCreateDevice failed ({result})");
    }
    result
}

/// The latest instance the engine made (for device-level lookups). The
/// engine makes a new one when it joins a game, after destroying the old
/// one: a write-once slot kept the dead instance and the loader aborted.
static INSTANCE: AtomicUsize = AtomicUsize::new(0);

fn instance() -> *mut c_void {
    INSTANCE.load(Ordering::Acquire) as *mut c_void
}

#[repr(C)]
struct SurfaceCaps {
    min_image_count: u32,
    max_image_count: u32,
    current_extent: [u32; 2],
    min_extent: [u32; 2],
    max_extent: [u32; 2],
    max_layers: u32,
    supported_transforms: u32,
    current_transform: u32,
    supported_alpha: u32,
    supported_usage: u32,
}

type SurfaceCapabilities = unsafe extern "C" fn(*mut c_void, u64, *mut SurfaceCaps) -> i32;

/// The host's answer, with the window's size where Wayland leaves it open.
unsafe extern "C" fn surface_capabilities(physical: *mut c_void, surface: u64, caps: *mut SurfaceCaps) -> i32 {
    let instance = instance();
    let Some(f) = host_proc(instance, c"vkGetPhysicalDeviceSurfaceCapabilitiesKHR") else {
        return -3;
    };
    // SAFETY: the host's function with that signature.
    let f: SurfaceCapabilities = unsafe { std::mem::transmute(f) };
    // SAFETY: the engine's arguments.
    let result = unsafe { f(physical, surface, caps) };
    if result == VK_SUCCESS
        && let Some((w, h)) = crate::window::native_window().map(|n| n.size())
    {
        // SAFETY: filled by the driver above.
        let caps = unsafe { &mut *caps };
        if caps.current_extent[0] == u32::MAX {
            caps.current_extent = [w as u32, h as u32];
        }
    }
    result
}

#[repr(C)]
struct SwapchainCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    surface: u64,
    min_image_count: u32,
    image_format: u32,
    image_color_space: u32,
    image_extent: [u32; 2],
    image_array_layers: u32,
    image_usage: u32,
    image_sharing_mode: u32,
    queue_family_index_count: u32,
    queue_family_indices: *const u32,
    pre_transform: u32,
    composite_alpha: u32,
    present_mode: u32,
    clipped: u32,
    old_swapchain: u64,
}

type CreateSwapchain = unsafe extern "C" fn(*mut c_void, *const SwapchainCreateInfo, *const c_void, *mut u64) -> i32;

/// The swapchain, presenting without waiting for the monitor (MAILBOX if
/// the driver has it for this surface, else IMMEDIATE, else as asked).
unsafe extern "C" fn create_swapchain(
    device: *mut c_void,
    info: *const SwapchainCreateInfo,
    allocator: *const c_void,
    swapchain: *mut u64,
) -> i32 {
    let instance = instance();
    let Some(f) = host_proc(instance, c"vkCreateSwapchainKHR") else {
        return -3;
    };
    // SAFETY: the host's function with that signature.
    let f: CreateSwapchain = unsafe { std::mem::transmute(f) };
    // SAFETY: the engine's valid create info.
    let mut copy = unsafe { std::ptr::read(info) };
    let asked = copy.present_mode;
    // VSync (runtime setting): keep the engine's FIFO.
    if !crate::prefs::get().vsync
        && let Some(mode) = no_wait_mode(copy.surface)
    {
        copy.present_mode = mode;
    }
    // Copying into the images (typed text, see vkoverlay.rs), if allowed.
    const TRANSFER: u32 = 0x1 | 0x2; // TRANSFER_SRC | TRANSFER_DST
    if supported_usage(copy.surface) & TRANSFER == TRANSFER {
        copy.image_usage |= TRANSFER;
    }
    eprintln!(
        "feline-runtime: vulkan: swapchain {}x{}, present mode {asked} → {}",
        copy.image_extent[0], copy.image_extent[1], copy.present_mode
    );
    // SAFETY: the engine's create info with a supported present mode.
    let result = unsafe { f(device, &copy, allocator, swapchain) };
    if result == VK_SUCCESS {
        type Images = unsafe extern "C" fn(*mut c_void, u64, *mut u32, *mut u64) -> i32;
        if let Some(g) = get_device_function(device, c"vkGetSwapchainImagesKHR") {
            // SAFETY: the host's function with that signature.
            let g: Images = unsafe { std::mem::transmute(g) };
            let mut n = 0u32;
            // SAFETY: counting call, then room for `n`, on the new swapchain.
            unsafe { g(device, *swapchain, &mut n, std::ptr::null_mut()) };
            let mut images = vec![0u64; n as usize];
            unsafe { g(device, *swapchain, &mut n, images.as_mut_ptr()) };
            images.truncate(n as usize);
            crate::vkoverlay::swapchain_made(
                device as u64,
                // SAFETY: filled by the driver.
                unsafe { *swapchain },
                images,
                copy.image_format as i32,
                copy.image_extent,
            );
        }
    }
    result
}

/// The usage flags swapchain images on `surface` may have.
fn supported_usage(surface: u64) -> u32 {
    let physical = PHYSICAL.load(Ordering::Acquire) as *mut c_void;
    let Some(f) = host_proc(instance(), c"vkGetPhysicalDeviceSurfaceCapabilitiesKHR") else {
        return 0;
    };
    // SAFETY: the host's function with that signature.
    let f: SurfaceCapabilities = unsafe { std::mem::transmute(f) };
    // SAFETY: an all-zero struct is valid to fill.
    let mut caps: SurfaceCaps = unsafe { std::mem::zeroed() };
    // SAFETY: the engine's physical device and surface.
    if unsafe { f(physical, surface, &mut caps) } == VK_SUCCESS {
        caps.supported_usage
    } else {
        0
    }
}

/// The host's device-level function `name`.
fn get_device_function(device: *mut c_void, name: &CStr) -> Pfn {
    let host_gdpa = host_proc(instance(), c"vkGetDeviceProcAddr")?;
    // SAFETY: the host's vkGetDeviceProcAddr.
    let host_gdpa: GetDeviceProcAddr = unsafe { std::mem::transmute(host_gdpa) };
    // SAFETY: a live device and a valid name.
    unsafe { host_gdpa(device, name.as_ptr()) }
}

type GetDeviceQueue = unsafe extern "C" fn(*mut c_void, u32, u32, *mut u64);
type GetDeviceQueue2 = unsafe extern "C" fn(*mut c_void, *const DeviceQueueInfo2, *mut u64);

#[repr(C)]
struct DeviceQueueInfo2 {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    family: u32,
    index: u32,
}

/// The host's queue, remembered with its family (for vkoverlay).
unsafe extern "C" fn get_device_queue(device: *mut c_void, family: u32, index: u32, queue: *mut u64) {
    let Some(f) = get_device_function(device, c"vkGetDeviceQueue") else {
        return;
    };
    // SAFETY: the host's function with that signature; the engine's args.
    let f: GetDeviceQueue = unsafe { std::mem::transmute(f) };
    unsafe { f(device, family, index, queue) };
    // SAFETY: filled by the driver.
    crate::vkoverlay::queue_made(unsafe { *queue }, device as u64, family);
}

unsafe extern "C" fn get_device_queue2(device: *mut c_void, info: *const DeviceQueueInfo2, queue: *mut u64) {
    let Some(f) = get_device_function(device, c"vkGetDeviceQueue2") else {
        return;
    };
    // SAFETY: the host's function with that signature; the engine's args.
    let f: GetDeviceQueue2 = unsafe { std::mem::transmute(f) };
    unsafe { f(device, info, queue) };
    // SAFETY: filled by the driver; the engine's valid info.
    crate::vkoverlay::queue_made(unsafe { *queue }, device as u64, unsafe { (*info).family });
}

/// MAILBOX or IMMEDIATE, if the surface supports one.
fn no_wait_mode(surface: u64) -> Option<u32> {
    type Modes = unsafe extern "C" fn(*mut c_void, u64, *mut u32, *mut u32) -> i32;
    let instance = instance();
    let physical = PHYSICAL.load(Ordering::Acquire) as *mut c_void;
    if instance.is_null() || physical.is_null() {
        return None;
    }
    let f = host_proc(instance, c"vkGetPhysicalDeviceSurfacePresentModesKHR")?;
    // SAFETY: the host's function with that signature.
    let f: Modes = unsafe { std::mem::transmute(f) };
    let mut n = 0u32;
    // SAFETY: counting call, then room for `n`.
    unsafe { f(physical, surface, &mut n, std::ptr::null_mut()) };
    let mut modes = vec![0u32; n as usize];
    unsafe { f(physical, surface, &mut n, modes.as_mut_ptr()) };
    modes.truncate(n as usize);
    [PRESENT_MODE_MAILBOX, PRESENT_MODE_IMMEDIATE]
        .into_iter()
        .find(|m| modes.contains(m))
}

/// The physical device the engine made its latest device on.
static PHYSICAL: AtomicUsize = AtomicUsize::new(0);

static HOST_PRESENT: AtomicUsize = AtomicUsize::new(0);
type QueuePresent = unsafe extern "C" fn(*mut c_void, *const c_void) -> i32;

/// Presents through the host, counting frames (logged like EGL's: the
/// first, then every power of two; FELINE_SWAP_LOG=1 adds the time per
/// frame every 64).
unsafe extern "C" fn queue_present(queue: *mut c_void, info: *const c_void) -> i32 {
    static FRAMES: AtomicUsize = AtomicUsize::new(0);
    static SINCE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let f = HOST_PRESENT.load(Ordering::Acquire);
    if f == 0 {
        return -3;
    }
    // SAFETY: the host's vkQueuePresentKHR.
    let f: QueuePresent = unsafe { std::mem::transmute(f) };
    // Typed text drawn into the image (vkoverlay.rs): then the present
    // waits on that instead of the engine's semaphores.
    // SAFETY: the engine's valid VkPresentInfoKHR.
    let present = unsafe { &*info.cast::<ash::vk::PresentInfoKHR>() };
    // SAFETY: as above.
    let mut drawn = unsafe { crate::vkoverlay::before_present(instance() as u64, queue as u64, present) };
    // A test screenshot (FELINE_INPUT_SCRIPT `shot`), after the overlay.
    if let Some(path) = crate::egl::take_screenshot_request() {
        let overlay_waits;
        let mut after_overlay = *present;
        if let Some(done) = drawn {
            overlay_waits = [done];
            after_overlay.wait_semaphore_count = 1;
            after_overlay.p_wait_semaphores = overlay_waits.as_ptr();
        }
        // SAFETY: the engine's present info (waiting on the overlay if drawn).
        if let Some(done) = unsafe { crate::vkoverlay::screenshot(instance() as u64, queue as u64, &after_overlay, &path) } {
            drawn = Some(done);
        }
    }
    let result = if let Some(done) = drawn {
        let waits = [done];
        let mut changed = *present;
        changed.wait_semaphore_count = 1;
        changed.p_wait_semaphores = waits.as_ptr();
        // SAFETY: the engine's present info with Feline's semaphore.
        unsafe { f(queue, std::ptr::from_ref(&changed).cast()) }
    } else {
        // SAFETY: the engine's queue and present info.
        unsafe { f(queue, info) }
    };
    let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || (n.is_power_of_two() && n >= 64) {
        eprintln!("feline-runtime: vulkan: frame {n} presented ({result})");
    }
    if n.is_multiple_of(64) && std::env::var_os("FELINE_SWAP_LOG").is_some() {
        let mut since = SINCE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(start) = *since {
            eprintln!(
                "feline-runtime: vulkan: 64 frames: {:.1} ms per frame",
                start.elapsed().as_secs_f64() * 1000.0 / 64.0
            );
        }
        *since = Some(std::time::Instant::now());
    }
    result
}
