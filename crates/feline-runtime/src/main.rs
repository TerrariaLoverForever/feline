// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! `feline-runtime`: Feline's own runtime (in development).
//!
//! So far `--check` (read the engine library and report what loading it
//! involves) and `--load` (also put it into memory and relocate it). Neither
//! runs any of its code.

use std::process::ExitCode;

const HELP: &str = "\
feline-runtime: Feline's own runtime for Roblox's Android client (in development)

Usage: feline-runtime --check <libroblox.so>
       feline-runtime --load <libroblox.so>
       feline-runtime --imports <libroblox.so>
       feline-runtime --init <libroblox.so>
       feline-runtime --natives <base.apk>

--check  reads the engine library and reports what loading it would involve.
--load   also puts it into memory: binds every import (forwarded to glibc
         or OpenGL ES, Feline data, or a stub), applies all relocations and
         sets memory permissions. Then it frees the memory and exits.
--imports  like --load, and lists every import by how it was bound.
--natives  lists the native methods declared in the APK's DEX files (names
         and type signatures only; no method bodies are read).
--init   like --load, then RUNS the engine's constructors (its start-up code),
         the first of its code to run. Stops at the first function Feline
         doesn't provide yet, and names it. Run it without network access,
         e.g. `unshare -rn feline-runtime --init …`.

Only --init runs any of the library's code. Roblox's code isn't disassembled;
only its ELF interface is read.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, path] if flag == "--check" => {
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match feline_runtime::check::check(&data) {
                Ok(report) => {
                    print!("{}", report.to_text());
                    if report.is_consistent() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(e) => {
                    eprintln!("feline-runtime: {path}: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        [flag, path] if flag == "--load" || flag == "--imports" => {
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match load_with_imports(&data, flag == "--imports") {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("feline-runtime: {path}: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        [flag, path] if flag == "--init" => {
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            // The APK next to the engine holds its assets.
            let apk = std::path::Path::new(path).with_file_name("base.apk");
            if let Err(e) = feline_runtime::assets::set_apk(&apk) {
                eprintln!("feline-runtime: no assets: {e}");
            }
            // The image and imports must stay alive while the engine's code
            // may run, so this never returns normally.
            run_init(&data)
        }
        [flag, path] if flag == "--natives" => {
            // Declarations only (see dex.rs): native methods' names and
            // type signatures from the APK's DEX files.
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match feline_runtime::dex::apk_native_methods(&data) {
                Ok(methods) => {
                    for m in &methods {
                        println!("{m}");
                    }
                    eprintln!(
                        "feline-runtime: {} native method declarations",
                        methods.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("feline-runtime: {path}: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        [flag] if flag == "-h" || flag == "--help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{HELP}");
            ExitCode::from(2)
        }
    }
}

/// Binds the imports, loads the library, and describes the result.
fn load_with_imports(data: &[u8], list: bool) -> Result<String, String> {
    use feline_runtime::imports::{self, Kind};
    use std::fmt::Write as _;

    let elf = feline_runtime::elf::Elf::parse(data).map_err(|e| e.to_string())?;
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let host = feline_runtime::host::Host::open();
    let imports = imports::bind(&symbols, &host)?;
    let (image, report) =
        feline_runtime::load::load(&elf, Some(&imports)).map_err(|e| e.to_string())?;

    let mut out = report.to_text(&image);
    let kinds = [
        Kind::ForwardLibc,
        Kind::ForwardLibm,
        Kind::ForwardGles,
        Kind::ForwardData,
        Kind::FelineData,
        Kind::Feline,
        Kind::Stub,
        Kind::WeakEmpty,
    ];
    let _ = writeln!(out, "Bound           {} imports:", imports.bindings.len());
    for kind in kinds {
        let _ = writeln!(out, "  {:>4}  {}", imports.count(kind), kind.label());
    }
    if !host.has_gles() {
        let _ = writeln!(
            out,
            "Note            no libGLESv2.so.2 found; OpenGL ES functions are stubs"
        );
    }
    if list {
        for kind in kinds {
            let names = imports.names(kind);
            if !names.is_empty() {
                let _ = writeln!(
                    out,
                    "\n{} ({}):\n  {}",
                    kind.label(),
                    names.len(),
                    names.join(" ")
                );
            }
        }
    }
    Ok(out)
}

/// `--init`: load, bind, run the constructors, report, and exit without
/// running the engine's exit handlers.
fn run_init(data: &[u8]) -> ExitCode {
    let result = (|| -> Result<String, String> {
        let elf = feline_runtime::elf::Elf::parse(data).map_err(|e| e.to_string())?;
        let symbols = elf.symbols().map_err(|e| e.to_string())?;
        let host = feline_runtime::host::Host::open();
        // Leaked on purpose: the engine's code and its pointers into Feline's
        // data must stay valid until the process ends.
        let imports = Box::leak(Box::new(feline_runtime::imports::bind(&symbols, &host)?));
        let (image, _) =
            feline_runtime::load::load(&elf, Some(imports)).map_err(|e| e.to_string())?;
        let image = Box::leak(Box::new(image));
        eprintln!(
            "feline-runtime: running {} constructors…",
            elf.init_array_len()
        );
        let ran = feline_runtime::init::run_constructors(&elf, image)?;
        // Diagnostic: FELINE_SKIP_ONLOAD=1 waits 3 s instead of calling
        // JNI_OnLoad, to tell its problems apart from the engine's threads'.
        if std::env::var_os("FELINE_SKIP_ONLOAD").is_some() {
            feline_runtime::init::start_thread_watch();
            eprintln!("feline-runtime: skipping JNI_OnLoad, waiting 3 s…");
            std::thread::sleep(std::time::Duration::from_secs(3));
            return Ok(format!(
                "Constructors    all {ran} ran and returned\nWaited          3 s without JNI_OnLoad, no crash\n"
            ));
        }
        eprintln!("feline-runtime: calling JNI_OnLoad…");
        let version = feline_runtime::init::call_jni_onload(&elf, image)?;
        // FELINE_START=1: continue like Android's GameActivity does (M4).
        let started = if std::env::var_os("FELINE_START").is_some() {
            eprintln!("feline-runtime: calling GameActivity.initializeNativeCode…");
            let handle = feline_runtime::init::call_initialize_native_code(&elf, image)?;
            // FELINE_WINDOW=1: open the window first (refused while a game
            // runs, so it can't take focus from it).
            let mut window = if std::env::var_os("FELINE_WINDOW").is_some() {
                if let Some(running) = runtime_running() {
                    return Err(format!(
                        "not opening a window while {running} is running (it could take focus from your game)"
                    ));
                }
                Some(feline_runtime::window::DesktopWindow::open()?)
            } else {
                None
            };
            // Then what Android does next: start, resume, and the main loop.
            eprintln!("feline-runtime: onStart, onResume, then the main loop…");
            feline_runtime::init::call_lifecycle("onStartNative", handle)?;
            feline_runtime::init::call_lifecycle("onResumeNative", handle)?;
            // FELINE_APP=1 (experimental): start Roblox's app the way its Java
            // code would, through NativeGLInterface. The order follows the
            // method names; Feline doesn't read Roblox's Java logic.
            if std::env::var_os("FELINE_APP").is_some() {
                use feline_runtime::init::call_gl_native;
                use feline_runtime::jni::{new_instance, new_string};
                // Real answers the engine depends on.
                let data = std::env::var_os("XDG_DATA_HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                            .join(".local/share")
                    });
                let assets =
                    feline_runtime::assets::unpack_assets(&data.join("feline/runtime/assets"))?;
                // The engine reads its TLS certificate bundle from
                // `./exe/ssl/cacert.pem` (relative to its data folder, where it
                // runs); the APK ships it as `assets/ssl/`. Link it there.
                link_certificates(&data.join("feline/runtime"), &assets)?;
                // The APK's assets hold `content/` (the asset folder) next to
                // `android/`, `fonts/`, …, which the engine finds relative to it.
                let assets = assets.join("content");
                let assets = assets.to_string_lossy();
                feline_runtime::jni::set_known_string(
                    "com/roblox/engine/jni/model/PlatformParams",
                    "assetFolderPath",
                    &assets,
                );
                feline_runtime::jni::set_known_string(
                    "com/roblox/engine/jni/autovalue/InitParams",
                    "baseURL",
                    "https://www.roblox.com/",
                );
                // Locale and theme from the desktop: LANG=en_US.UTF-8 -> "en-US" / "en_us".
                let lang = std::env::var("LANG").unwrap_or_default();
                let mut parts = lang.split(['.', '@']).next().unwrap_or("").split('_');
                let language = parts
                    .next()
                    .filter(|l| l.len() == 2)
                    .unwrap_or("en")
                    .to_ascii_lowercase();
                let country = parts
                    .next()
                    .filter(|c| c.len() == 2)
                    .unwrap_or("US")
                    .to_ascii_uppercase();
                let locale_class = "com/roblox/engine/jni/locale/NativeLocaleJavaInterface";
                let roblox_locale = format!("{language}_{}", country.to_ascii_lowercase());
                feline_runtime::jni::set_known_string(
                    locale_class,
                    "getLocale",
                    &format!("{language}-{country}"),
                );
                feline_runtime::jni::set_known_string(
                    locale_class,
                    "getRobloxLocale",
                    &roblox_locale,
                );
                feline_runtime::jni::set_known_string(
                    locale_class,
                    "getGameLocale",
                    &roblox_locale,
                );
                feline_runtime::jni::set_known_string(
                    "com/roblox/engine/jni/user/NativeUserJavaInterface",
                    "getTheme",
                    "dark",
                );
                call_gl_native(&elf, image, "nativeGameGlobalInit", &[])?;
                let empty = || new_string("") as usize;
                // Client settings ("FastFlags") as JSON. The engine accepts an
                // empty `applicationSettings` object (returns 0; "" or "{}"
                // return 1) and keeps its built-in defaults. Fetching Roblox's
                // real settings comes later. FELINE_SETTINGS overrides it.
                let settings = std::env::var("FELINE_SETTINGS")
                    .unwrap_or_else(|_| r#"{"applicationSettings":{}}"#.to_owned());
                let first = new_string(&settings) as usize;
                let r = call_gl_native(
                    &elf,
                    image,
                    "nativeInitClientSettings",
                    &[first, empty(), empty()],
                )?;
                eprintln!(
                    "feline-runtime: nativeInitClientSettings returned {}",
                    r as i32
                );
                let init = new_instance("com/roblox/engine/jni/autovalue/InitParams") as usize;
                call_gl_native(&elf, image, "nativeAppBridgeV2InitWithParams", &[init])?;
                let start = new_instance("com/roblox/engine/jni/autovalue/StartAppParams") as usize;
                call_gl_native(&elf, image, "nativeAppBridgeV2StartAppWithParams", &[start])?;
                if window.is_some() {
                    let platform =
                        new_instance("com/roblox/engine/jni/model/PlatformParams") as usize;
                    let surface = feline_runtime::window::surface_object() as usize;
                    call_gl_native(
                        &elf,
                        image,
                        "nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams",
                        &[surface, platform],
                    )?;
                }
            }
            let lifecycle = match window.as_mut() {
                Some(window) => {
                    // FELINE_SECONDS: how long the window stays open (15 s).
                    let seconds = std::env::var("FELINE_SECONDS")
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .filter(|&s| s > 0)
                        .unwrap_or(15);
                    let opened = std::time::Instant::now();
                    let why = feline_runtime::init::run_window_loop(
                        window,
                        handle,
                        std::time::Duration::from_secs(seconds),
                    )?;
                    let open = opened.elapsed().as_secs_f64().max(1.0);
                    let frames = feline_runtime::egl::frames();
                    format!(
                        "window open {open:.0} s with the engine ({why}), {frames} frames presented (about {:.0} per second)",
                        frames as f64 / open
                    )
                }
                None => {
                    let callbacks =
                        feline_runtime::init::run_main_loop(std::time::Duration::from_secs(5));
                    format!("main looper ran 5 s, {callbacks} callbacks")
                }
            };
            format!(
                "initializeNativeCode returned handle {handle:#x}\n\
                 Lifecycle       onStart, onResume; {lifecycle}\n"
            )
        } else {
            String::new()
        };
        let world = feline_runtime::jni::summary();
        let mut per_class: std::collections::BTreeMap<&str, usize> = Default::default();
        for (class, _, _) in &world.natives {
            *per_class.entry(class.as_str()).or_insert(0) += 1;
        }
        let mut text = format!(
            "Constructors    all {ran} ran and returned\n\
             JNI_OnLoad      returned version {version:#x}\n\
             Java world      {} classes looked up, {} methods/fields, {} natives registered\n",
            world.classes.len(),
            world.members,
            world.natives.len()
        );
        for (class, n) in per_class {
            text.push_str(&format!("  natives {n:>4}  {class}\n"));
        }
        text.push_str(&started);
        Ok(text)
    })();
    match result {
        Ok(text) => {
            print!("{text}");
            feline_runtime::init::exit_now(0)
        }
        Err(e) => {
            eprintln!("feline-runtime: {e}");
            feline_runtime::init::exit_now(1)
        }
    }
}

/// The name of a running Roblox runtime (Sober, Cordial, Stud), if any:
/// test windows aren't opened then, so they can't take focus from a game.
fn runtime_running() -> Option<String> {
    const NAMES: &[&str] = &[
        "sober",
        "cordial",
        "cordial-run",
        "stud",
        "stud-runtime-bi",
        "stud-render-hos",
    ];
    let entries = std::fs::read_dir("/proc").ok()?;
    entries.filter_map(Result::ok).find_map(|entry| {
        let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
        let comm = comm.trim();
        NAMES.contains(&comm).then(|| comm.to_owned())
    })
}

/// Makes `<runtime>/files/exe/ssl` point to the unpacked `<assets>/ssl`.
fn link_certificates(runtime: &std::path::Path, assets: &std::path::Path) -> Result<(), String> {
    let exe = runtime.join("files/exe");
    std::fs::create_dir_all(&exe).map_err(|e| format!("couldn't create {}: {e}", exe.display()))?;
    let link = exe.join("ssl");
    let target = assets.join("ssl");
    if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
        return Ok(());
    }
    // Replace whatever is there (an old link); never a real folder's files.
    if link.is_symlink() {
        let _ = std::fs::remove_file(&link);
    }
    std::os::unix::fs::symlink(&target, &link).map_err(|e| {
        format!(
            "couldn't link {} to {}: {e}",
            link.display(),
            target.display()
        )
    })
}
