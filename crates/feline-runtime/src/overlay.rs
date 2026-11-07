// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! The text of a focused text box, drawn over the engine's frame.
//!
//! While Android's keyboard edits a text box, the engine leaves the box's
//! text to Android (a Java text field on top of the game view) and shows
//! only the box. Feline has no Java views, so it draws the text itself, just
//! before each frame is presented: the box's text area is covered with the
//! box's own background color (read from the frame beside the text) and the
//! typed text drawn on it in Roblox's font (Builder Sans, from the APK), at
//! the box's size and color, with a caret. Password boxes show bullets.
//! (mocktail draws a similar overlay for the same reason.)
//!
//! The drawing is one textured rectangle. Every piece of GL state it changes
//! is saved first and restored after, so the engine's next frame is drawn
//! as if nothing happened.
//!
//! # `unsafe` in this module
//!
//! GL ES calls on the engine's context, current on the presenting thread.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_void};
use std::sync::{Mutex, OnceLock};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

use crate::text::BoxInfo;

type Enum = u32;
type Uint = u32;
type Int = i32;

const GL_TEXTURE_2D: Enum = 0x0DE1;
const GL_TEXTURE0: Enum = 0x84C0;
const GL_RGBA: Enum = 0x1908;
const GL_UNSIGNED_BYTE: Enum = 0x1401;
const GL_FLOAT: Enum = 0x1406;
const GL_TRIANGLE_STRIP: Enum = 0x0005;
const GL_ARRAY_BUFFER: Enum = 0x8892;
const GL_FRAMEBUFFER: Enum = 0x8D40;
const GL_TEXTURE_MIN_FILTER: Enum = 0x2801;
const GL_TEXTURE_MAG_FILTER: Enum = 0x2800;
const GL_TEXTURE_WRAP_S: Enum = 0x2802;
const GL_TEXTURE_WRAP_T: Enum = 0x2803;
const GL_NEAREST: Int = 0x2600;
const GL_CLAMP_TO_EDGE: Int = 0x812F;
const GL_UNPACK_ALIGNMENT: Enum = 0x0CF5;
const GL_UNPACK_ROW_LENGTH: Enum = 0x0CF2;
const GL_PIXEL_UNPACK_BUFFER: Enum = 0x88EC;
const GL_VERTEX_SHADER: Enum = 0x8B31;
const GL_FRAGMENT_SHADER: Enum = 0x8B30;
const GL_BLEND: Enum = 0x0BE2;
const GL_DEPTH_TEST: Enum = 0x0B71;
const GL_CULL_FACE: Enum = 0x0B44;
const GL_SCISSOR_TEST: Enum = 0x0C11;
const GL_STENCIL_TEST: Enum = 0x0B90;
const GL_CURRENT_PROGRAM: Enum = 0x8B8D;
const GL_ACTIVE_TEXTURE: Enum = 0x84E0;
const GL_TEXTURE_BINDING_2D: Enum = 0x8069;
const GL_ARRAY_BUFFER_BINDING: Enum = 0x8894;
const GL_VERTEX_ARRAY_BINDING: Enum = 0x85B5;
const GL_FRAMEBUFFER_BINDING: Enum = 0x8CA6;
const GL_VIEWPORT: Enum = 0x0BA2;
const GL_COLOR_WRITEMASK: Enum = 0x0C23;
const GL_PIXEL_UNPACK_BUFFER_BINDING: Enum = 0x88EF;

/// The GL ES functions the overlay uses, from the host driver.
#[allow(clippy::type_complexity)]
struct Gl {
    get_integerv: unsafe extern "C" fn(Enum, *mut Int),
    get_booleanv: unsafe extern "C" fn(Enum, *mut u8),
    is_enabled: unsafe extern "C" fn(Enum) -> u8,
    enable: unsafe extern "C" fn(Enum),
    disable: unsafe extern "C" fn(Enum),
    color_mask: unsafe extern "C" fn(u8, u8, u8, u8),
    viewport: unsafe extern "C" fn(Int, Int, Int, Int),
    read_pixels: unsafe extern "C" fn(Int, Int, Int, Int, Enum, Enum, *mut c_void),
    create_shader: unsafe extern "C" fn(Enum) -> Uint,
    shader_source: unsafe extern "C" fn(Uint, Int, *const *const c_char, *const Int),
    compile_shader: unsafe extern "C" fn(Uint),
    create_program: unsafe extern "C" fn() -> Uint,
    attach_shader: unsafe extern "C" fn(Uint, Uint),
    bind_attrib_location: unsafe extern "C" fn(Uint, Uint, *const c_char),
    link_program: unsafe extern "C" fn(Uint),
    use_program: unsafe extern "C" fn(Uint),
    get_uniform_location: unsafe extern "C" fn(Uint, *const c_char) -> Int,
    uniform1i: unsafe extern "C" fn(Int, Int),
    gen_textures: unsafe extern "C" fn(Int, *mut Uint),
    bind_texture: unsafe extern "C" fn(Enum, Uint),
    active_texture: unsafe extern "C" fn(Enum),
    tex_parameteri: unsafe extern "C" fn(Enum, Enum, Int),
    tex_image_2d: unsafe extern "C" fn(Enum, Int, Int, Int, Int, Int, Enum, Enum, *const c_void),
    pixel_storei: unsafe extern "C" fn(Enum, Int),
    bind_buffer: unsafe extern "C" fn(Enum, Uint),
    bind_framebuffer: unsafe extern "C" fn(Enum, Uint),
    bind_vertex_array: unsafe extern "C" fn(Uint),
    vertex_attrib_pointer: unsafe extern "C" fn(Uint, Int, Enum, u8, Int, *const c_void),
    enable_vertex_attrib_array: unsafe extern "C" fn(Uint),
    disable_vertex_attrib_array: unsafe extern "C" fn(Uint),
    draw_arrays: unsafe extern "C" fn(Enum, Int, Int),
}

fn gl() -> Option<&'static Gl> {
    static GL: OnceLock<Option<Gl>> = OnceLock::new();
    GL.get_or_init(|| {
        use crate::egl::gl_function as f;
        Some(Gl {
            get_integerv: f("glGetIntegerv")?,
            get_booleanv: f("glGetBooleanv")?,
            is_enabled: f("glIsEnabled")?,
            enable: f("glEnable")?,
            disable: f("glDisable")?,
            color_mask: f("glColorMask")?,
            viewport: f("glViewport")?,
            read_pixels: f("glReadPixels")?,
            create_shader: f("glCreateShader")?,
            shader_source: f("glShaderSource")?,
            compile_shader: f("glCompileShader")?,
            create_program: f("glCreateProgram")?,
            attach_shader: f("glAttachShader")?,
            bind_attrib_location: f("glBindAttribLocation")?,
            link_program: f("glLinkProgram")?,
            use_program: f("glUseProgram")?,
            get_uniform_location: f("glGetUniformLocation")?,
            uniform1i: f("glUniform1i")?,
            gen_textures: f("glGenTextures")?,
            bind_texture: f("glBindTexture")?,
            active_texture: f("glActiveTexture")?,
            tex_parameteri: f("glTexParameteri")?,
            tex_image_2d: f("glTexImage2D")?,
            pixel_storei: f("glPixelStorei")?,
            bind_buffer: f("glBindBuffer")?,
            bind_framebuffer: f("glBindFramebuffer")?,
            bind_vertex_array: f("glBindVertexArray")?,
            vertex_attrib_pointer: f("glVertexAttribPointer")?,
            enable_vertex_attrib_array: f("glEnableVertexAttribArray")?,
            disable_vertex_attrib_array: f("glDisableVertexAttribArray")?,
            draw_arrays: f("glDrawArrays")?,
        })
    })
    .as_ref()
}

/// Roblox's UI font, from the APK's assets (unpacked at startup).
fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join(".local/share")
            });
        let path = data.join("feline/runtime/assets/content/fonts/BuilderSans-Regular.otf");
        let bytes = std::fs::read(&path).ok()?;
        let font = FontVec::try_from_vec(bytes).ok();
        if font.is_none() {
            eprintln!("feline-runtime: overlay: couldn't read {}", path.display());
        }
        font
    })
    .as_ref()
}

/// The program (one texture, no transform) and the texture, made once.
struct Resources {
    program: Uint,
    texture: Uint,
}

fn resources(gl: &Gl) -> Option<&'static Resources> {
    static RESOURCES: OnceLock<Option<Resources>> = OnceLock::new();
    RESOURCES
        .get_or_init(|| {
            const VERTEX: &str = "attribute vec2 p; attribute vec2 t; varying vec2 v;\n\
                void main() { v = t; gl_Position = vec4(p, 0.0, 1.0); }\0";
            const FRAGMENT: &str = "precision mediump float; varying vec2 v; uniform sampler2D s;\n\
                void main() { gl_FragColor = texture2D(s, v); }\0";
            // SAFETY: GL calls on the current context with valid sources.
            unsafe {
                let shader = |kind, source: &str| {
                    let s = (gl.create_shader)(kind);
                    let text = source.as_ptr().cast::<c_char>();
                    (gl.shader_source)(s, 1, &text, std::ptr::null());
                    (gl.compile_shader)(s);
                    s
                };
                let program = (gl.create_program)();
                (gl.attach_shader)(program, shader(GL_VERTEX_SHADER, VERTEX));
                (gl.attach_shader)(program, shader(GL_FRAGMENT_SHADER, FRAGMENT));
                (gl.bind_attrib_location)(program, 0, c"p".as_ptr());
                (gl.bind_attrib_location)(program, 1, c"t".as_ptr());
                (gl.link_program)(program);
                let mut texture = 0;
                (gl.gen_textures)(1, &mut texture);
                (program != 0 && texture != 0).then_some(Resources { program, texture })
            }
        })
        .as_ref()
}

/// The last image drawn, so text is only laid out again when it changes.
struct Image {
    key: (String, bool, i32, i32, i32, i32),
    width: i32,
    height: i32,
    rgba: Vec<u8>,
    background: [u8; 3],
}

static LAST: Mutex<Option<Image>> = Mutex::new(None);

/// Lays out `text` over `background` in a `width`×`height` RGBA image.
fn render(
    info: &BoxInfo,
    text: &str,
    selected: bool,
    width: i32,
    height: i32,
    background: [u8; 3],
) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        rgba.extend_from_slice(&[background[0], background[1], background[2], 255]);
    }
    let Some(font) = font() else {
        return rgba;
    };
    let shown: String = if crate::text::is_password(info.input_type) {
        "•".repeat(text.chars().count())
    } else {
        text.to_owned()
    };
    let scale = PxScale::from(info.font_size.max(8.0));
    let scaled = font.as_scaled(scale);
    // Advance widths, to keep the end of long text (and the caret) visible.
    let advance: f32 = shown
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum();
    let caret_room = 2.0;
    let free = width as f32 - advance - caret_room;
    let start_x = match info.x_alignment {
        // TextXAlignment: Left 0, Right 1, Center 2.
        1 => free.max(0.0).min(free),
        2 => (free / 2.0).max(if free < 0.0 { free } else { 0.0 }),
        _ => free.min(0.0),
    };
    let baseline = (height as f32 + scaled.ascent() + scaled.descent()) / 2.0;
    // Ctrl+A: the selected text gets Android's selection highlight.
    if selected && advance > 0.0 {
        let (from, to) = (
            start_x.max(0.0) as i32,
            (start_x + advance).min(width as f32) as i32,
        );
        let top = ((height as f32 - info.font_size * 1.2) / 2.0).max(0.0) as i32;
        for y in top..height - top {
            for x in from..to {
                let at = ((y * width + x) * 4) as usize;
                rgba[at..at + 3].copy_from_slice(&[0x33, 0x5f, 0xb0]);
            }
        }
    }
    let color = info.text_color;
    let ink = [(color >> 16) as u8, (color >> 8) as u8, color as u8];
    let mut put = |x: i32, y: i32, coverage: f32| {
        if x < 0 || y < 0 || x >= width || y >= height {
            return;
        }
        let at = ((y * width + x) * 4) as usize;
        for c in 0..3 {
            let bg = f32::from(rgba[at + c]);
            rgba[at + c] = (bg + (f32::from(ink[c]) - bg) * coverage.clamp(0.0, 1.0)) as u8;
        }
    };
    let mut pen = start_x;
    for c in shown.chars() {
        let id = font.glyph_id(c);
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(pen, baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                put(
                    bounds.min.x as i32 + gx as i32,
                    bounds.min.y as i32 + gy as i32,
                    coverage,
                );
            });
        }
        pen += scaled.h_advance(id);
    }
    // A steady caret after the text (no blinking).
    let caret_x = pen.round() as i32;
    let top = ((height as f32 - info.font_size) / 2.0).max(0.0) as i32;
    for y in top..(height - top).max(top + 1) {
        put(caret_x, y, 1.0);
    }
    rgba
}

/// Draws the focused text box's text into the frame about to be presented
/// (`surface_width`×`surface_height` pixels). Does nothing without one.
pub fn draw(surface_width: i32, surface_height: i32) {
    let Some((info, text, selected)) = crate::text::shown() else {
        *LAST.lock().unwrap_or_else(|p| p.into_inner()) = None;
        return;
    };
    let Some(gl) = gl() else { return };
    let (x, y) = (info.x.round() as i32, info.y.round() as i32);
    let (width, height) = (info.width.round() as i32, info.height.round() as i32);
    if width <= 0 || height <= 0 || surface_width <= 0 || surface_height <= 0 {
        return;
    }
    // GL's rows count from the bottom.
    let gl_y = surface_height - y - height;
    let Some(res) = resources(gl) else { return };

    // SAFETY: GL calls on the engine's current context; every state changed
    // is saved here and restored below.
    unsafe {
        let int = |name| {
            let mut v = 0;
            (gl.get_integerv)(name, &mut v);
            v
        };
        let program = int(GL_CURRENT_PROGRAM);
        let active = int(GL_ACTIVE_TEXTURE);
        let vertex_array = int(GL_VERTEX_ARRAY_BINDING);
        let array_buffer = int(GL_ARRAY_BUFFER_BINDING);
        let framebuffer = int(GL_FRAMEBUFFER_BINDING);
        let unpack_buffer = int(GL_PIXEL_UNPACK_BUFFER_BINDING);
        let mut viewport = [0; 4];
        (gl.get_integerv)(GL_VIEWPORT, viewport.as_mut_ptr());
        let mut mask = [1u8; 4];
        (gl.get_booleanv)(GL_COLOR_WRITEMASK, mask.as_mut_ptr());
        let caps = [
            GL_BLEND,
            GL_DEPTH_TEST,
            GL_CULL_FACE,
            GL_SCISSOR_TEST,
            GL_STENCIL_TEST,
        ];
        let enabled: Vec<u8> = caps.iter().map(|&c| (gl.is_enabled)(c)).collect();

        (gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
        (gl.bind_buffer)(GL_PIXEL_UNPACK_BUFFER, 0);
        for &c in &caps {
            (gl.disable)(c);
        }
        (gl.color_mask)(1, 1, 1, 1);

        // The box's background: the frame's color just left of the text.
        let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
        let key = (text.clone(), selected, x, y, width, height);
        if last.as_ref().is_none_or(|l| l.key != key) {
            let background = last
                .as_ref()
                .filter(|l| l.key.2 == x && l.key.3 == y)
                .map_or_else(
                    || {
                        let mut px = [0u8; 4];
                        (gl.read_pixels)(
                            (x - 3).max(0),
                            gl_y + height / 2,
                            1,
                            1,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            px.as_mut_ptr().cast(),
                        );
                        [px[0], px[1], px[2]]
                    },
                    |l| l.background,
                );
            *last = Some(Image {
                rgba: render(&info, &text, selected, width, height, background),
                key,
                width,
                height,
                background,
            });
        }
        let image = last.as_ref().expect("set above");

        let texture = int(GL_TEXTURE_BINDING_2D);
        (gl.active_texture)(GL_TEXTURE0);
        let texture0 = int(GL_TEXTURE_BINDING_2D);
        (gl.bind_texture)(GL_TEXTURE_2D, res.texture);
        for (name, value) in [
            (GL_TEXTURE_MIN_FILTER, GL_NEAREST),
            (GL_TEXTURE_MAG_FILTER, GL_NEAREST),
            (GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE),
            (GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE),
        ] {
            (gl.tex_parameteri)(GL_TEXTURE_2D, name, value);
        }
        let alignment = int(GL_UNPACK_ALIGNMENT);
        let row_length = int(GL_UNPACK_ROW_LENGTH);
        (gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 4);
        (gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, 0);
        (gl.tex_image_2d)(
            GL_TEXTURE_2D,
            0,
            GL_RGBA as Int,
            image.width,
            image.height,
            0,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            image.rgba.as_ptr().cast(),
        );

        (gl.viewport)(x, gl_y, width, height);
        (gl.use_program)(res.program);
        let sampler = (gl.get_uniform_location)(res.program, c"s".as_ptr());
        (gl.uniform1i)(sampler, 0);
        (gl.bind_vertex_array)(0);
        (gl.bind_buffer)(GL_ARRAY_BUFFER, 0);
        // The whole viewport; the image's first row is the box's top.
        let positions: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        let coords: [f32; 8] = [0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0];
        (gl.vertex_attrib_pointer)(0, 2, GL_FLOAT, 0, 0, positions.as_ptr().cast());
        (gl.vertex_attrib_pointer)(1, 2, GL_FLOAT, 0, 0, coords.as_ptr().cast());
        (gl.enable_vertex_attrib_array)(0);
        (gl.enable_vertex_attrib_array)(1);
        (gl.draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
        (gl.disable_vertex_attrib_array)(0);
        (gl.disable_vertex_attrib_array)(1);

        // Restore everything.
        (gl.pixel_storei)(GL_UNPACK_ALIGNMENT, alignment);
        (gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, row_length);
        (gl.bind_texture)(GL_TEXTURE_2D, texture0 as Uint);
        (gl.active_texture)(active as Enum);
        (gl.bind_texture)(GL_TEXTURE_2D, texture as Uint);
        (gl.use_program)(program as Uint);
        (gl.bind_vertex_array)(vertex_array as Uint);
        (gl.bind_buffer)(GL_ARRAY_BUFFER, array_buffer as Uint);
        (gl.bind_buffer)(GL_PIXEL_UNPACK_BUFFER, unpack_buffer as Uint);
        (gl.bind_framebuffer)(GL_FRAMEBUFFER, framebuffer as Uint);
        (gl.viewport)(viewport[0], viewport[1], viewport[2], viewport[3]);
        (gl.color_mask)(mask[0], mask[1], mask[2], mask[3]);
        for (&c, &on) in caps.iter().zip(&enabled) {
            if on != 0 {
                (gl.enable)(c);
            }
        }
    }
}
