// SPDX-License-Identifier: GPL-3.0-or-later
//! The desk cat on Wayland (Linux).
//!
//! Wayland doesn't let an ordinary window place itself or see the pointer
//! outside its own windows. The cat uses what desktops offer instead:
//!
//! - **Its window:** a small *layer-shell* surface (`zwlr_layer_shell_v1`,
//!   which KDE Plasma, Hyprland, Sway, niri, Wayfire, COSMIC and others
//!   have; GNOME doesn't) on the overlay layer, moved around by its margins
//!   from the top-left corner of the screen it's on. Its input region is
//!   empty, so every click goes through it.
//! - **The pointer:** on KDE Plasma, a small KWin script (loaded through
//!   KWin's D-Bus scripting interface) reads `workspace.cursorPos` 20 times
//!   a second and calls Feline back over D-Bus when it moved; on Hyprland,
//!   Hyprland's `cursorpos` socket command. Elsewhere the cat can't see the
//!   pointer, so it wanders around the screen on its own.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use feline_core::cat::{Cat, FRAME, Pose, TICK, render};
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::WaylandSurface,
    shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};

use super::Look;

/// Whether this session can show the cat: Wayland with layer-shell.
pub fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let Ok(conn) = Connection::connect_to_env() else {
            return false;
        };
        let Ok((globals, _queue)) = registry_queue_init::<Probe>(&conn) else {
            return false;
        };
        globals
            .contents()
            .with_list(|list| list.iter().any(|g| g.interface == "zwlr_layer_shell_v1"))
    })
}

/// Whether the cat will see the pointer here (KDE Plasma, Hyprland).
pub fn follows_pointer() -> bool {
    kde() || hyprland_socket().is_some()
}

fn kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("KDE")))
}

/// For [`supported`]'s registry check (it handles no events).
struct Probe;

impl smithay_client_toolkit::reexports::client::Dispatch<
    smithay_client_toolkit::reexports::client::protocol::wl_registry::WlRegistry,
    smithay_client_toolkit::reexports::client::globals::GlobalListContents,
> for Probe
{
    fn event(
        _: &mut Self,
        _: &smithay_client_toolkit::reexports::client::protocol::wl_registry::WlRegistry,
        _: smithay_client_toolkit::reexports::client::protocol::wl_registry::Event,
        _: &smithay_client_toolkit::reexports::client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// ---------------------------------------------------------------------------
// Where the pointer is
// ---------------------------------------------------------------------------

enum Pointer {
    KWin(KWinWatch),
    Hyprland(PathBuf),
    Unknown,
}

impl Pointer {
    fn find() -> Self {
        if kde() {
            match KWinWatch::start() {
                Ok(watch) => return Self::KWin(watch),
                Err(e) => eprintln!("feline: the desk cat can't see the pointer through KWin ({e}); it wanders instead"),
            }
        }
        if let Some(socket) = hyprland_socket() {
            return Self::Hyprland(socket);
        }
        Self::Unknown
    }

    /// The pointer, in the desktop's logical coordinates.
    fn position(&self) -> Option<(f32, f32)> {
        match self {
            Self::KWin(watch) => *watch.at.lock().unwrap_or_else(|p| p.into_inner()),
            Self::Hyprland(socket) => hyprland_cursor(socket),
            Self::Unknown => None,
        }
    }
}

const BUS_PATH: &str = "/DeskCat";
const BUS_INTERFACE: &str = "io.github.TerrariaLoverForever.Feline.DeskCat";
const KWIN_PLUGIN: &str = "feline-desk-cat";

/// Feline's end of the KWin script: it receives the pointer.
struct DeskCatBus {
    at: Arc<Mutex<Option<(f32, f32)>>>,
}

#[zbus::interface(name = "io.github.TerrariaLoverForever.Feline.DeskCat")]
impl DeskCatBus {
    /// "x,y" (a string, so the numbers' D-Bus type can't go wrong).
    fn pointer(&self, at: &str) {
        let parsed = at
            .split_once(',')
            .and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)));
        if let Some(at) = parsed {
            *self.at.lock().unwrap_or_else(|p| p.into_inner()) = Some(at);
        }
    }
}

/// A KWin script that tells Feline where the pointer is; unloaded on drop.
struct KWinWatch {
    bus: zbus::blocking::Connection,
    at: Arc<Mutex<Option<(f32, f32)>>>,
    script: PathBuf,
}

impl KWinWatch {
    fn start() -> Result<Self, String> {
        let at = Arc::new(Mutex::new(None));
        let bus = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.serve_at(BUS_PATH, DeskCatBus { at: at.clone() }))
            .and_then(|b| b.build())
            .map_err(|e| format!("no D-Bus session: {e}"))?;
        let name = bus.unique_name().map(|n| n.to_string()).ok_or("no D-Bus name")?;
        // Unique names are like ":1.234"; anything else stays out of the script.
        if !name.chars().all(|c| c == ':' || c == '.' || c.is_ascii_digit()) {
            return Err(format!("unexpected D-Bus name {name}"));
        }
        let script = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("feline-desk-cat-{}.js", std::process::id()));
        std::fs::write(&script, kwin_script(&name)).map_err(|e| format!("couldn't write the script: {e}"))?;
        let watch = Self { bus, at, script };
        // A script left over from a Feline that didn't close properly.
        watch.unload();
        let path = watch.script.to_string_lossy().into_owned();
        let id: i32 = watch
            .bus
            .call_method(
                Some("org.kde.KWin"),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "loadScript",
                &(path.as_str(), KWIN_PLUGIN),
            )
            .map_err(|e| format!("KWin's scripting: {e}"))?
            .body()
            .deserialize()
            .map_err(|e| e.to_string())?;
        if id < 0 {
            return Err("KWin didn't load the script".into());
        }
        watch
            .bus
            .call_method(
                Some("org.kde.KWin"),
                format!("/Scripting/Script{id}").as_str(),
                Some("org.kde.kwin.Script"),
                "run",
                &(),
            )
            .map_err(|e| format!("KWin didn't run the script: {e}"))?;
        Ok(watch)
    }

    fn unload(&self) {
        let _ = self.bus.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(KWIN_PLUGIN,),
        );
    }
}

impl Drop for KWinWatch {
    fn drop(&mut self) {
        self.unload();
        let _ = std::fs::remove_file(&self.script);
    }
}

/// The KWin script: checks the pointer 20 times a second, sends it when it
/// moved. `service` is Feline's unique D-Bus name.
fn kwin_script(service: &str) -> String {
    format!(
        "// Feline's desk cat: tells Feline where the pointer is.\n\
         var last = \"\";\n\
         var timer = new QTimer();\n\
         timer.interval = 50;\n\
         timer.timeout.connect(function () {{\n\
         \x20   var p = workspace.cursorPos;\n\
         \x20   var at = p.x + \",\" + p.y;\n\
         \x20   if (at !== last) {{\n\
         \x20       last = at;\n\
         \x20       callDBus(\"{service}\", \"{BUS_PATH}\", \"{BUS_INTERFACE}\", \"Pointer\", at);\n\
         \x20   }}\n\
         }});\n\
         timer.start();\n"
    )
}

/// Hyprland's command socket, if this is Hyprland.
fn hyprland_socket() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let socket = PathBuf::from(runtime).join("hypr").join(signature).join(".socket.sock");
    socket.exists().then_some(socket)
}

/// Hyprland's `cursorpos`: "x, y".
fn hyprland_cursor(socket: &std::path::Path) -> Option<(f32, f32)> {
    use std::io::{Read, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(50))).ok()?;
    stream.write_all(b"cursorpos").ok()?;
    let mut reply = String::new();
    stream.take(64).read_to_string(&mut reply).ok()?;
    let (x, y) = reply.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Where the cat strolls when it can't see the pointer: a new spot now and
/// then, somewhere on the screen.
struct Wander {
    seed: u64,
    target: Option<(f32, f32)>,
    until: Instant,
}

impl Wander {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9, |d| d.as_nanos() as u64)
            | 1;
        Self {
            seed,
            target: None,
            until: Instant::now(),
        }
    }

    fn next(&mut self) -> u64 {
        // xorshift64
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        self.seed
    }

    fn target(&mut self, area: Rect) -> (f32, f32) {
        if self.target.is_none() || Instant::now() >= self.until {
            let margin = 80.0_f32.min(area.w as f32 / 4.0).min(area.h as f32 / 4.0);
            let span = |n: u64, len: i32| margin + (n % 10_000) as f32 / 10_000.0 * (len as f32 - 2.0 * margin);
            let (a, b, c) = (self.next(), self.next(), self.next());
            self.target = Some((area.x as f32 + span(a, area.w), area.y as f32 + span(b, area.h)));
            // Long enough to get there, wash and nap.
            self.until = Instant::now() + Duration::from_secs(15 + c % 40);
        }
        self.target.unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// The cat's window
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32 && y >= self.y as f32 && x < (self.x + self.w) as f32 && y < (self.y + self.h) as f32
    }

    /// How far (x, y) is from this rectangle (0 inside).
    fn distance(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x as f32 - x).max(x - (self.x + self.w) as f32).max(0.0);
        let dy = (self.y as f32 - y).max(y - (self.y + self.h) as f32).max(0.0);
        dx.hypot(dy)
    }
}

/// A screen: its output, its rectangle in the desktop's logical
/// coordinates, and its scale.
type Screen = (wl_output::WlOutput, Rect, i32);

/// The surface the cat is drawn on, on one screen.
struct CatSurface {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    /// Logical size and the screen's scale it was made for.
    size: u32,
    scale: i32,
    configured: bool,
    /// What's on it: (pose, look), and its margins (left, top).
    drawn: Option<(Pose, Look)>,
    margin: Option<(i32, i32)>,
}

struct Desk {
    registry: RegistryState,
    compositor: CompositorState,
    outputs: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    pool: SlotPool,
    surface: Option<CatSurface>,
    /// The compositor closed our surface (e.g. its screen went away).
    closed: bool,
}

impl Desk {
    /// Each screen's rectangle in the desktop's logical coordinates, and
    /// its scale.
    fn screens(&self) -> Vec<Screen> {
        self.outputs
            .outputs()
            .filter_map(|output| {
                let info = self.outputs.info(&output)?;
                let (x, y) = info.logical_position.unwrap_or(info.location);
                let (w, h) = info.logical_size.or_else(|| {
                    let mode = info.modes.iter().find(|m| m.current)?;
                    let scale = info.scale_factor.max(1);
                    Some((mode.dimensions.0 / scale, mode.dimensions.1 / scale))
                })?;
                (w > 0 && h > 0).then_some((output, Rect { x, y, w, h }, info.scale_factor.max(1)))
            })
            .collect()
    }

    /// Puts the cat (top-left `at`, logical) on a screen, showing `pose`.
    fn show(&mut self, qh: &QueueHandle<Self>, on: &Screen, at: (i32, i32), pose: Pose, look: Look) {
        let (output, screen, scale) = (&on.0, on.1, on.2);
        let size = FRAME * look.scale;
        if self.closed {
            self.surface = None;
            self.closed = false;
        }
        // Another screen, size or scale: a new surface.
        if self
            .surface
            .as_ref()
            .is_some_and(|s| s.output != *output || s.size != size || s.scale != scale)
        {
            self.surface = None;
        }
        let margin = (at.0 - screen.x, at.1 - screen.y);
        if self.surface.is_none() {
            let wl = self.compositor.create_surface(qh);
            // Clicks go through: an empty input region.
            if let Ok(region) = Region::new(&self.compositor) {
                wl.set_input_region(Some(region.wl_region()));
            }
            let layer = self.layer_shell.create_layer_surface(
                qh,
                wl,
                Layer::Overlay,
                Some("feline-desk-cat"),
                Some(output),
            );
            layer.set_anchor(Anchor::TOP | Anchor::LEFT);
            layer.set_size(size, size);
            // Ignore panels: the margins count from the screen's corner.
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_margin(margin.1, 0, 0, margin.0);
            layer.commit();
            self.surface = Some(CatSurface {
                layer,
                output: output.clone(),
                size,
                scale,
                configured: false,
                drawn: None,
                margin: Some(margin),
            });
        }
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if !surface.configured {
            return;
        }
        let mut changed = false;
        if surface.margin != Some(margin) {
            surface.layer.set_margin(margin.1, 0, 0, margin.0);
            surface.margin = Some(margin);
            changed = true;
        }
        if surface.drawn != Some((pose, look)) {
            let pixels = (size as i32) * scale;
            let image = render(pose, look.coat, look.collar, look.scale * scale as u32);
            match self.pool.create_buffer(pixels, pixels, pixels * 4, wl_shm::Format::Argb8888) {
                Ok((buffer, canvas)) => {
                    // Argb8888 is premultiplied B, G, R, A in memory.
                    for (out, px) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(image.pixels.as_chunks::<4>().0) {
                        let a = u32::from(px[3]);
                        let pre = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
                        out.copy_from_slice(&[pre(px[2]), pre(px[1]), pre(px[0]), px[3]]);
                    }
                    let wl = surface.layer.wl_surface();
                    let _ = surface.layer.set_buffer_scale(scale as u32);
                    wl.damage_buffer(0, 0, pixels, pixels);
                    if buffer.attach_to(wl).is_ok() {
                        surface.drawn = Some((pose, look));
                        changed = true;
                    }
                }
                Err(e) => eprintln!("feline: the desk cat couldn't be drawn: {e}"),
            }
        }
        if changed {
            surface.layer.commit();
        }
    }
}

/// The cat's thread on Wayland: one step and one picture every tick until
/// stopped.
pub fn live(stop: &AtomicBool, shared_look: &Mutex<Look>) {
    if let Err(e) = run(stop, shared_look) {
        eprintln!("feline: the desk cat couldn't come out: {e}");
    }
}

fn run(stop: &AtomicBool, shared_look: &Mutex<Look>) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland: {e}"))?;
    let (globals, queue) = registry_queue_init::<Desk>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| e.to_string())?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| "this desktop has no layer-shell windows")?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| e.to_string())?;
    let pool = SlotPool::new(64 * 64 * 4, &shm).map_err(|e| e.to_string())?;
    let mut desk = Desk {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        shm,
        layer_shell,
        pool,
        surface: None,
        closed: false,
    };
    let mut event_loop: EventLoop<Desk> = EventLoop::try_new().map_err(|e| e.to_string())?;
    WaylandSource::new(conn, queue)
        .insert(event_loop.handle())
        .map_err(|e| e.error.to_string())?;
    // The screens arrive in the first round trips.
    for _ in 0..3 {
        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut desk)
            .map_err(|e| e.to_string())?;
    }

    let pointer = Pointer::find();
    let started = Instant::now();
    let mut wander = Wander::new();
    let mut cat: Option<Cat> = None;
    let mut next_tick = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let wait = next_tick.saturating_duration_since(Instant::now());
        event_loop
            .dispatch(Some(wait), &mut desk)
            .map_err(|e| e.to_string())?;
        if Instant::now() < next_tick {
            continue;
        }
        next_tick = Instant::now() + TICK;
        let look = *shared_look.lock().unwrap_or_else(|p| p.into_inner());
        let screens = desk.screens();
        let Some(first) = screens.first().map(|s| s.1) else {
            continue;
        };
        let size = (FRAME * look.scale) as f32;
        let target = pointer.position();
        // KWin's first report takes a moment: wait for it before the cat
        // comes out, so it starts near the pointer.
        if cat.is_none() && target.is_none() && matches!(pointer, Pointer::KWin(_)) && started.elapsed() < Duration::from_secs(1) {
            continue;
        }
        // It strolls in from a little below the pointer (or the middle).
        let cat = cat.get_or_insert_with(|| {
            let (x, y) = target.unwrap_or((
                (first.x + first.w / 2) as f32,
                (first.y + first.h / 2) as f32,
            ));
            Cat::new(x - 160.0, y + 120.0)
        });
        // The screen the cat is on (or the nearest).
        let on = |x: f32, y: f32| {
            screens
                .iter()
                .find(|s| s.1.contains(x, y))
                .or_else(|| screens.iter().min_by(|a, b| a.1.distance(x, y).total_cmp(&b.1.distance(x, y))))
        };
        let target = target.unwrap_or_else(|| wander.target(on(cat.x, cat.y).map_or(first, |s| s.1)));
        let pose = cat.step(target, size);
        let Some(here) = on(cat.x, cat.y).cloned() else {
            continue;
        };
        let screen = here.1;
        // Keep the whole cat on that screen.
        let half = size / 2.0;
        cat.x = cat.x.clamp(screen.x as f32 + half, (screen.x + screen.w) as f32 - half);
        cat.y = cat.y.clamp(screen.y as f32 + half, (screen.y + screen.h) as f32 - half);
        let at = ((cat.x - half).round() as i32, (cat.y - half).round() as i32);
        desk.show(&qh, &here, at, pose, look);
    }
    // Dropping the surface (and the KWin script) takes the cat home.
    desk.surface = None;
    let _ = event_loop.dispatch(Some(Duration::ZERO), &mut desk);
    drop(pointer);
    Ok(())
}

impl CompositorHandler for Desk {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {
    }
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {
    }
}

impl OutputHandler for Desk {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.surface.as_ref().is_some_and(|s| s.output == output) {
            self.surface = None;
        }
    }
}

impl LayerShellHandler for Desk {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Some(surface) = self.surface.as_mut().filter(|s| s.layer == *layer) {
            surface.configured = true;
        }
    }
}

impl ShmHandler for Desk {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Desk {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Desk);
delegate_output!(Desk);
delegate_shm!(Desk);
delegate_layer!(Desk);
delegate_registry!(Desk);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kwin_script_names_feline() {
        let script = kwin_script(":1.42");
        assert!(script.contains("callDBus(\":1.42\", \"/DeskCat\""));
        assert!(script.contains("workspace.cursorPos"));
    }

    #[test]
    fn wandering_stays_on_the_screen() {
        let mut wander = Wander::new();
        let screen = Rect { x: 1920, y: 0, w: 1280, h: 1024 };
        for _ in 0..50 {
            wander.until = Instant::now();
            let (x, y) = wander.target(screen);
            assert!(screen.contains(x, y), "{x},{y}");
        }
    }

    /// Lets a real cat out for a few seconds (needs a Wayland desktop):
    /// `cargo test -p feline -- --ignored lets_the_cat_out --nocapture`.
    #[test]
    #[ignore = "opens a real window on the desktop"]
    fn lets_the_cat_out() {
        assert!(supported(), "no Wayland layer-shell here");
        let stop = Arc::new(AtomicBool::new(false));
        let look = Arc::new(Mutex::new(Look {
            coat: feline_core::cat::Coat::Ginger,
            scale: 3,
            collar: 0x8f7ef7,
        }));
        let thread = {
            let (stop, look) = (stop.clone(), look.clone());
            std::thread::spawn(move || run(&stop, &look))
        };
        let seconds = std::env::var("FELINE_CAT_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
        std::thread::sleep(Duration::from_secs(seconds));
        stop.store(true, Ordering::Relaxed);
        thread.join().unwrap().unwrap();
    }

    #[test]
    fn nearest_screen() {
        let a = Rect { x: 0, y: 0, w: 100, h: 100 };
        assert_eq!(a.distance(50.0, 50.0), 0.0);
        assert_eq!(a.distance(130.0, 50.0), 30.0);
    }
}
