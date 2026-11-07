// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! The focused text box's typed text under Vulkan (see `overlay.rs` for why
//! Feline draws it). Under OpenGL it's drawn at `eglSwapBuffers`; here it's
//! copied into the swapchain image just before `vkQueuePresentKHR`: one
//! small command buffer that waits for the engine's frame, reads the pixel
//! left of the box (the box's background, used on the next frame), and
//! writes the box's text image; the present then waits for it. Nothing runs
//! while no text box is focused.
//!
//! Unsafe: Vulkan calls on the engine's device and queue, through `ash`'s
//! function tables loaded from the desktop driver.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use ash::vk::{self, Handle};

/// What's known about the engine's Vulkan objects.
#[derive(Default)]
struct World {
    /// queue → (device, queue family)
    queues: HashMap<u64, (u64, u32)>,
    /// device → physical device
    physicals: HashMap<u64, u64>,
    /// swapchain → its images and format
    swapchains: HashMap<u64, Swap>,
    /// device → Feline's objects on it
    contexts: HashMap<u64, Context>,
    /// The last text image, and the background read for its position.
    last: Option<Last>,
}

struct Swap {
    device: u64,
    images: Vec<vk::Image>,
    format: vk::Format,
    extent: vk::Extent2D,
}

struct Last {
    key: (String, bool, i32, i32, i32, i32),
    rgba: Vec<u8>,
    background: [u8; 3],
}

struct Context {
    device: ash::Device,
    family: u32,
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    done: vk::Semaphore,
    fence: vk::Fence,
    /// Host-visible buffer: the text image, then 4 bytes for the readback.
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut u8,
    size: u64,
}

// SAFETY: the raw pointers are only used under WORLD's lock.
unsafe impl Send for Context {}

static WORLD: Mutex<Option<World>> = Mutex::new(None);

fn world<T>(f: impl FnOnce(&mut World) -> T) -> T {
    let mut guard = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(World::default))
}

pub fn queue_made(queue: u64, device: u64, family: u32) {
    world(|w| w.queues.insert(queue, (device, family)));
}

pub fn device_made(device: u64, physical: u64) {
    world(|w| w.physicals.insert(device, physical));
}

pub fn swapchain_made(device: u64, swapchain: u64, images: Vec<u64>, format: i32, extent: [u32; 2]) {
    world(|w| {
        w.swapchains.insert(
            swapchain,
            Swap {
                device,
                images: images.into_iter().map(vk::Image::from_raw).collect(),
                format: vk::Format::from_raw(format),
                extent: vk::Extent2D {
                    width: extent[0],
                    height: extent[1],
                },
            },
        )
    });
}

/// The driver's function tables for the engine's instance and `device`.
fn tables(instance: u64, device: u64) -> Option<(ash::Instance, ash::Device)> {
    let gipa = crate::vulkan::host_get_instance_proc_addr()?;
    // SAFETY: the driver's vkGetInstanceProcAddr (C and "system" are the
    // same ABI on x86_64 Linux).
    let static_fn = ash::StaticFn {
        get_instance_proc_addr: unsafe { std::mem::transmute::<usize, vk::PFN_vkGetInstanceProcAddr>(gipa) },
    };
    // SAFETY: live handles the engine made through the same driver.
    let instance = unsafe { ash::Instance::load(&static_fn, vk::Instance::from_raw(instance)) };
    let device = unsafe { ash::Device::load(instance.fp_v1_0(), vk::Device::from_raw(device)) };
    Some((instance, device))
}

impl Context {
    /// Makes Feline's objects on `device` (queue family `family`).
    fn new(instance: u64, device: u64, physical: u64, family: u32, size: u64) -> Option<Self> {
        let (instance, dev) = tables(instance, device)?;
        // SAFETY: Vulkan calls on the engine's live device, with valid info.
        unsafe {
            let pool = dev
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .ok()?;
            let commands = dev
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .ok()?[0];
            let done = dev.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).ok()?;
            let fence = dev.create_fence(&vk::FenceCreateInfo::default(), None).ok()?;
            let buffer = dev
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size)
                        .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST),
                    None,
                )
                .ok()?;
            let needs = dev.get_buffer_memory_requirements(buffer);
            let props = instance.get_physical_device_memory_properties(vk::PhysicalDevice::from_raw(physical));
            let wanted = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let kind = (0..props.memory_type_count).find(|&i| {
                needs.memory_type_bits & (1 << i) != 0
                    && props.memory_types[i as usize].property_flags.contains(wanted)
            })?;
            let memory = dev
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind),
                    None,
                )
                .ok()?;
            dev.bind_buffer_memory(buffer, memory, 0).ok()?;
            let mapped = dev.map_memory(memory, 0, size, vk::MemoryMapFlags::empty()).ok()?.cast::<u8>();
            Some(Self {
                device: dev,
                family,
                pool,
                commands,
                done,
                fence,
                buffer,
                memory,
                mapped,
                size,
            })
        }
    }

    fn destroy(self) {
        // SAFETY: Feline's own objects, idle (each use waits for its fence).
        unsafe {
            self.device.unmap_memory(self.memory);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_semaphore(self.done, None);
            self.device.destroy_command_pool(self.pool, None);
        }
    }
}

/// Before presenting on `queue`: draws the focused text box into the image
/// being presented. The semaphore the present must wait on instead of the
/// engine's, if it drew.
///
/// # Safety
/// `info` is the engine's valid `VkPresentInfoKHR`.
pub unsafe fn before_present(instance: u64, queue: u64, info: &vk::PresentInfoKHR) -> Option<vk::Semaphore> {
    let (boxinfo, text, selected) = crate::text::shown()?;
    if info.swapchain_count == 0 {
        return None;
    }
    // SAFETY: the engine's present info lists at least one swapchain.
    let (swapchain, index) = unsafe { (*info.p_swapchains, *info.p_image_indices) };
    let mut guard = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    let w = guard.get_or_insert_with(World::default);
    let swap = w.swapchains.get(&swapchain.as_raw())?;
    let image = *swap.images.get(index as usize)?;
    let (format, extent, device) = (swap.format, swap.extent, swap.device);
    let &(qdevice, family) = w.queues.get(&queue)?;
    if qdevice != device {
        return None;
    }
    // The box, inside the image.
    let (x, y) = (boxinfo.x.round() as i32, boxinfo.y.round() as i32);
    let (bw, bh) = (boxinfo.width.round() as i32, boxinfo.height.round() as i32);
    let (iw, ih) = (extent.width as i32, extent.height as i32);
    if bw <= 0 || bh <= 0 || x < 0 || y < 0 || x + bw > iw || y + bh > ih {
        return None;
    }
    let bytes = (bw * bh * 4) as u64;
    let need = bytes + 4;
    if w.contexts.get(&device).is_some_and(|c| c.size < need || c.family != family)
        && let Some(old) = w.contexts.remove(&device)
    {
        old.destroy();
    }
    if !w.contexts.contains_key(&device) {
        let physical = *w.physicals.get(&device)?;
        let ctx = Context::new(instance, device, physical, family, need.max(256 * 1024))?;
        w.contexts.insert(device, ctx);
    }

    // The background: read on an earlier frame for this position.
    let key = (text.clone(), selected, x, y, bw, bh);
    let background = w
        .last
        .as_ref()
        .filter(|l| l.key.2 == x && l.key.3 == y)
        .map_or([0x20, 0x22, 0x26], |l| l.background);
    let ctx = w.contexts.get(&device)?;
    let readback = {
        // SAFETY: the last 4 bytes of the mapped buffer, written by the
        // previous overlay frame (waited for).
        let px = unsafe { std::slice::from_raw_parts(ctx.mapped.add((ctx.size - 4) as usize), 4) };
        let bgra = matches!(format, vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB);
        if bgra { [px[2], px[1], px[0]] } else { [px[0], px[1], px[2]] }
    };
    let have_readback = w.last.as_ref().is_some_and(|l| l.key.2 == x && l.key.3 == y);
    let background = if have_readback { readback } else { background };
    if w.last.as_ref().is_none_or(|l| l.key != key || l.background != background) {
        w.last = Some(Last {
            rgba: crate::overlay::render(&boxinfo, &text, selected, bw, bh, background),
            key,
            background,
        });
    }
    let rgba = &w.last.as_ref()?.rgba;
    let ctx = w.contexts.get(&device)?;
    let bgra = matches!(format, vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB);
    // SAFETY: the mapped buffer holds at least `bytes` (+4) bytes, and no
    // GPU work uses it (the previous use was waited for).
    unsafe {
        let out = std::slice::from_raw_parts_mut(ctx.mapped, bytes as usize);
        for (o, i) in out.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
            *o = if bgra { [i[2], i[1], i[0], 255] } else { [i[0], i[1], i[2], 255] };
        }
    }
    let dev = &ctx.device;
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    let layers = vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1);
    // SAFETY: Feline's command buffer on the engine's device; the image is
    // the one being presented, which the engine finished rendering (the
    // submit waits on its present semaphores).
    unsafe {
        dev.reset_command_buffer(ctx.commands, vk::CommandBufferResetFlags::empty()).ok()?;
        dev.begin_command_buffer(
            ctx.commands,
            &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .ok()?;
        let to_general = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        dev.cmd_pipeline_barrier(
            ctx.commands,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_general],
        );
        // The pixel left of the box, for the next frame's background.
        dev.cmd_copy_image_to_buffer(
            ctx.commands,
            image,
            vk::ImageLayout::GENERAL,
            ctx.buffer,
            &[vk::BufferImageCopy::default()
                .buffer_offset(ctx.size - 4)
                .image_subresource(layers)
                .image_offset(vk::Offset3D {
                    x: (x - 3).max(0),
                    y: y + bh / 2,
                    z: 0,
                })
                .image_extent(vk::Extent3D {
                    width: 1,
                    height: 1,
                    depth: 1,
                })],
        );
        let between = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        dev.cmd_pipeline_barrier(
            ctx.commands,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[between],
            &[],
            &[],
        );
        dev.cmd_copy_buffer_to_image(
            ctx.commands,
            ctx.buffer,
            image,
            vk::ImageLayout::GENERAL,
            &[vk::BufferImageCopy::default()
                .image_subresource(layers)
                .image_offset(vk::Offset3D { x, y, z: 0 })
                .image_extent(vk::Extent3D {
                    width: bw as u32,
                    height: bh as u32,
                    depth: 1,
                })],
        );
        let to_present = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        dev.cmd_pipeline_barrier(
            ctx.commands,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_present],
        );
        dev.end_command_buffer(ctx.commands).ok()?;
        let waits = std::slice::from_raw_parts(info.p_wait_semaphores, info.wait_semaphore_count as usize);
        let stages = vec![vk::PipelineStageFlags::TRANSFER; waits.len()];
        let commands = [ctx.commands];
        let signals = [ctx.done];
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(waits)
            .wait_dst_stage_mask(&stages)
            .command_buffers(&commands)
            .signal_semaphores(&signals);
        dev.queue_submit(vk::Queue::from_raw(queue), &[submit], ctx.fence).ok()?;
        // Only while typing: wait, so the buffer and command buffer can be
        // reused next frame (and the readback is there).
        dev.wait_for_fences(&[ctx.fence], true, u64::MAX).ok()?;
        dev.reset_fences(&[ctx.fence]).ok()?;
    }
    Some(ctx.done)
}

/// FELINE_INPUT_SCRIPT's `shot` under Vulkan: saves the image about to be
/// presented as a binary PPM. Waits for the GPU (tests only).
///
/// # Safety
/// `info` is the engine's valid `VkPresentInfoKHR`.
pub unsafe fn screenshot(instance: u64, queue: u64, info: &vk::PresentInfoKHR, path: &str) -> Option<vk::Semaphore> {
    if info.swapchain_count == 0 {
        return None;
    }
    // SAFETY: the engine's present info lists at least one swapchain.
    let (swapchain, index) = unsafe { (*info.p_swapchains, *info.p_image_indices) };
    let (image, format, extent, device, family, physical) = world(|w| {
        let swap = w.swapchains.get(&swapchain.as_raw())?;
        let &(_, family) = w.queues.get(&queue)?;
        Some((
            *swap.images.get(index as usize)?,
            swap.format,
            swap.extent,
            swap.device,
            family,
            *w.physicals.get(&swap.device)?,
        ))
    })?;
    let size = u64::from(extent.width) * u64::from(extent.height) * 4;
    let ctx = Context::new(instance, device, physical, family, size)?;
    let dev = &ctx.device;
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    let layers = vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1);
    // SAFETY: Feline's own objects on the engine's device; waits for the
    // engine's frame first.
    let ok = unsafe {
        let run = || -> Option<()> {
            dev.begin_command_buffer(ctx.commands, &vk::CommandBufferBeginInfo::default()).ok()?;
            let barrier = |old, new| {
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                    .dst_access_mask(vk::AccessFlags::MEMORY_READ)
                    .old_layout(old)
                    .new_layout(new)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range)
            };
            dev.cmd_pipeline_barrier(
                ctx.commands,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier(vk::ImageLayout::PRESENT_SRC_KHR, vk::ImageLayout::GENERAL)],
            );
            dev.cmd_copy_image_to_buffer(
                ctx.commands,
                image,
                vk::ImageLayout::GENERAL,
                ctx.buffer,
                &[vk::BufferImageCopy::default()
                    .image_subresource(layers)
                    .image_extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })],
            );
            dev.cmd_pipeline_barrier(
                ctx.commands,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier(vk::ImageLayout::GENERAL, vk::ImageLayout::PRESENT_SRC_KHR)],
            );
            dev.end_command_buffer(ctx.commands).ok()?;
            let waits = std::slice::from_raw_parts(info.p_wait_semaphores, info.wait_semaphore_count as usize);
            let stages = vec![vk::PipelineStageFlags::TRANSFER; waits.len()];
            let commands = [ctx.commands];
            let signals = [ctx.done];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(waits)
                .wait_dst_stage_mask(&stages)
                .command_buffers(&commands)
                .signal_semaphores(&signals);
            dev.queue_submit(vk::Queue::from_raw(queue), &[submit], ctx.fence).ok()?;
            dev.wait_for_fences(&[ctx.fence], true, u64::MAX).ok()?;
            Some(())
        };
        run().is_some()
    };
    if !ok {
        return None;
    }
    // SAFETY: the mapped buffer holds the whole image, copied above.
    let pixels = unsafe { std::slice::from_raw_parts(ctx.mapped, size as usize) };
    let bgra = matches!(format, vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB);
    let mut ppm = format!("P6\n{} {}\n255\n", extent.width, extent.height).into_bytes();
    for px in pixels.as_chunks::<4>().0 {
        if bgra {
            ppm.extend_from_slice(&[px[2], px[1], px[0]]);
        } else {
            ppm.extend_from_slice(&px[..3]);
        }
    }
    let _ = std::fs::write(path, ppm);
    eprintln!("feline-runtime: vulkan: screenshot saved to {path}");
    // The present waits on Feline's semaphore: the context isn't destroyed
    // (tests only), so its semaphore stays valid.
    Some(ctx.done)
}
