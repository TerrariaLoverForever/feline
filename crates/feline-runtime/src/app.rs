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

Usage: feline-runtime --play <libroblox.so>
       feline-runtime --check <libroblox.so>
       feline-runtime --load <libroblox.so>
       feline-runtime --imports <libroblox.so>
       feline-runtime --init <libroblox.so>
       feline-runtime --natives <base.apk>
       feline-runtime --login
       feline-runtime --version
       feline-runtime --save-cookie   (reads the cookie from standard input)
       feline-runtime --logout

--play   starts Roblox's app in a window, until you close it (what the
         Feline launcher runs). base.apk must be next to libroblox.so.
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

--login  opens Feline's login window (Roblox's website, feline-login) and
         saves the session, so the app starts logged in. With --play it also
         opens when you press Sign In or Create Account in the app, and the
         app restarts logged in (FELINE_NO_LOGIN=1 turns that off).
--save-cookie  saves a Roblox session cookie (.ROBLOSECURITY) so the app
         starts logged in; readable only by you. --logout forgets it.

Only --init runs any of the library's code. Roblox's code isn't disassembled;
only its ELF interface is read.
";

pub fn main() -> ExitCode {
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
        [flag, path] if flag == "--init" || flag == "--play" => {
            if flag == "--play" {
                PLAY.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = ENGINE_PATH.set(std::path::PathBuf::from(path));
            // Proxy (runtime setting), before any thread starts.
            feline_runtime::net::use_proxy(&feline_runtime::prefs::get().proxy);
            if flag == "--play" {
                feline_runtime::versions::started(std::path::Path::new(path));
            }
            let data = match feline_runtime::load::map_file(std::path::Path::new(path)) {
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
            // The login window opens when the app shows its Sign In or
            // Create Account screen (FELINE_NO_LOGIN=1: never).
            if std::env::var_os("FELINE_NO_LOGIN").is_none()
                && let Ok(data) = runtime_data()
            {
                feline_runtime::login::set_runtime_data(data);
            }
            // The image and imports must stay alive while the engine's code
            // may run, so this never returns normally.
            run_init(data)
        }
        [flag] if flag == "--version" => {
            println!("feline-runtime {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        [flag] if flag == "--login" => match runtime_data().and_then(|d| feline_runtime::login::open_window(&d, false)) {
            Ok(path) => {
                println!("Logged in: the session is saved in {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("feline-runtime: {e}");
                ExitCode::FAILURE
            }
        },
        [flag] if flag == "--save-cookie" => {
            let mut text = String::new();
            if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
                eprintln!("feline-runtime: couldn't read the cookie: {e}");
                return ExitCode::FAILURE;
            }
            let saved = runtime_data().and_then(|d| feline_runtime::login::save_cookie(&d, &text));
            text.clear();
            match saved {
                Ok(path) => {
                    println!("Logged in: the session is saved in {}", path.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("feline-runtime: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        [flag] if flag == "--logout" => match runtime_data() {
            Ok(d) => {
                feline_runtime::login::forget_cookie(&d);
                println!("Logged out: the saved session is gone");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("feline-runtime: {e}");
                ExitCode::FAILURE
            }
        },
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
/// The engine file being run (`--play`/`--init`), for `load_mapped`.
static ENGINE_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

fn run_init(data: &'static [u8]) -> ExitCode {
    let result = (|| -> Result<String, String> {
        let elf = feline_runtime::elf::Elf::parse(data).map_err(|e| e.to_string())?;
        let symbols = elf.symbols().map_err(|e| e.to_string())?;
        let host = feline_runtime::host::Host::open();
        // Leaked on purpose: the engine's code and its pointers into Feline's
        // data must stay valid until the process ends.
        let imports = Box::leak(Box::new(feline_runtime::imports::bind(&symbols, &host)?));
        // The engine's code is mapped from its file (see `load_mapped`).
        let (image, _) = match ENGINE_PATH.get().and_then(|p| std::fs::File::open(p).ok()) {
            Some(file) => feline_runtime::load::load_mapped(&elf, Some(imports), &file),
            None => feline_runtime::load::load(&elf, Some(imports)),
        }
        .map_err(|e| e.to_string())?;
        let image = Box::leak(Box::new(image));
        // The engine's segments are copied into the image: the file's
        // pages aren't needed in memory any more.
        feline_runtime::load::release_file_pages(data);
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
        describe_device();
        eprintln!("feline-runtime: calling JNI_OnLoad…");
        let version = feline_runtime::init::call_jni_onload(&elf, image)?;
        // The engine's cache folder (Android's Context.getCacheDir()), set
        // right after loading the engine, before the activity starts. With
        // Roblox's real client settings the engine requires it ("Path does
        // not exist").
        if wanted("FELINE_APP") {
            let cache = runtime_data()?.join("cache");
            std::fs::create_dir_all(&cache)
                .map_err(|e| format!("couldn't create {}: {e}", cache.display()))?;
            feline_runtime::init::call_java_native(
                &elf,
                image,
                "com/roblox/engine/jni/NativeSettingsInterface",
                "nativeSetCacheDirectory",
                true,
                &[feline_runtime::jni::new_string(&cache.to_string_lossy()) as usize],
            )?;
            // The device's details (Android's Java code passes a
            // DeviceParams here at startup). Without it the Lua app sees no
            // OS version and shows "Roblox no longer supports Android 6 and 7".
            feline_runtime::init::call_java_native(
                &elf,
                image,
                "com/roblox/engine/jni/NativeSettingsInterface",
                "nativeSetDeviceInfo",
                true,
                &[
                    feline_runtime::jni::new_instance("com/roblox/engine/jni/model/DeviceParams")
                        as usize,
                ],
            )?;
            session_step(&elf, image, "early")?;
            // Roblox's own input functions, for the window's mouse and keys.
            feline_runtime::init::prepare_input_natives(&elf, image)?;
            // FELINE_STORAGE_INIT=1 (experiment): LocalStorageManager's
            // initStorageManagerNativeV3(AssetManager, String, String), here
            // with the files and cache folders (the parameter names aren't
            // in the declaration; a guess, which didn't change the
            // HttpClient crash rate).
            if std::env::var_os("FELINE_STORAGE_INIT").is_some() {
                let files = runtime_data()?.join("files");
                feline_runtime::init::call_java_native(
                    &elf,
                    image,
                    "com/roblox/client/LocalStorageManager",
                    "initStorageManagerNativeV3",
                    false,
                    &[
                        feline_runtime::jni::new_instance("android/content/res/AssetManager")
                            as usize,
                        feline_runtime::jni::new_string(&files.to_string_lossy()) as usize,
                        feline_runtime::jni::new_string(&cache.to_string_lossy()) as usize,
                    ],
                )?;
            }
        }
        // FELINE_START=1: continue like Android's GameActivity does (M4).
        let started = if wanted("FELINE_START") {
            if let Ok(data) = runtime_data() {
                freshen_flag_cache(&data.join("cache/cache"));
            }
            eprintln!("feline-runtime: calling GameActivity.initializeNativeCode…");
            let handle = feline_runtime::init::call_initialize_native_code(&elf, image)?;
            // FELINE_WINDOW=1: open the window first (refused while a game
            // runs, so it can't take focus from it).
            let mut window = if wanted("FELINE_WINDOW") {
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
            if wanted("FELINE_APP") {
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
                    &feline_runtime::prefs::get().theme,
                );
                // The platform games are told, as the Android app's Java side
                // answers (empty showed as "unknown" in games): Android, or
                // Windows with the "Windows PC" graphics optimisation.
                let platform = feline_runtime::prefs::get().platform_name();
                eprintln!("feline-runtime: games are told the platform is {platform}");
                feline_runtime::jni::set_known_string(
                    "com/roblox/engine/jni/user/NativeUserJavaInterface",
                    "getPlatformName",
                    platform,
                );
                // The FPS limit: Roblox's own Maximum Frame Rate setting.
                // Graphics quality: Roblox's own setting, if one is chosen.
                let quality = feline_runtime::prefs::get().graphics_quality;
                if quality > 0 {
                    feline_runtime::prefs::write_graphics_quality(
                        &data.join("feline/runtime/files/appData"),
                        quality,
                    );
                }
                let prefs = feline_runtime::prefs::get();
                if prefs.fps_limit > 0 {
                    // Feline paces frames itself: the engine's limit opens
                    // up so the two don't fight (see pacing.rs).
                    let cap = if feline_runtime::pacing::paces(prefs) {
                        feline_runtime::prefs::MAX_FPS_LIMIT
                    } else {
                        prefs.fps_limit
                    };
                    feline_runtime::prefs::write_framerate_cap(
                        &data.join("feline/runtime/files/appData"),
                        cap,
                    );
                }
                call_gl_native(&elf, image, "nativeGameGlobalInit", &[])?;
                let empty = || new_string("") as usize;
                // Client settings ("FastFlags") as JSON. The engine accepts an
                // empty `applicationSettings` object (returns 0; "" or "{}"
                // return 1) and keeps its built-in defaults. Fetching Roblox's
                // real settings comes later. FELINE_SETTINGS overrides it.
                let runtime = data.join("feline/runtime");
                let settings = client_settings(&runtime);
                // Roblox's desktop layout instead of the tablet one.
                let settings = match feline_runtime::desktop::apply(&runtime, USER_ID.get().copied()) {
                    Some(policy) => feline_runtime::desktop::add_to_settings(&settings, &policy),
                    None => settings,
                };
                // Roblox's PC in-game settings (Maximum Frame Rate).
                let settings = feline_runtime::prefs::add_flags(
                    &settings,
                    &feline_runtime::prefs::get().flags(),
                );
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
                session_step(&elf, image, "starting")?;
                let start = new_instance("com/roblox/engine/jni/autovalue/StartAppParams") as usize;
                call_gl_native(&elf, image, "nativeAppBridgeV2StartAppWithParams", &[start])?;
                session_step(&elf, image, "started")?;
                feline_runtime::init::remember_app_surface_update(&elf, image)?;
                feline_runtime::init::remember_refresh_natives(&elf, image)?;
                if let Err(e) = feline_runtime::gamepad::remember_natives(&elf, image) {
                    eprintln!("feline-runtime: no controllers: {e}");
                }
                // The app's questions to the platform (microphone permission).
                if let Some(env) = feline_runtime::jni::current_env()
                    && let Err(e) = feline_runtime::messagebus::start(&elf, image, env)
                {
                    eprintln!("feline-runtime: message bus: {e}");
                }
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
                    // FELINE_SECONDS: how long the window stays open (15 s;
                    // with --play, until it's closed).
                    let seconds = std::env::var("FELINE_SECONDS")
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .filter(|&s| s > 0)
                        .unwrap_or(if playing() { UNTIL_CLOSED } else { 15 });
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

/// The name of a running Roblox runtime (Sober, Cordial, Stud, Tipsy,
/// Mocktail or Nightcap), if any:
/// test windows aren't opened then, so they can't take focus from a game.
fn runtime_running() -> Option<String> {
    const NAMES: &[&str] = &[
        "sober",
        "cordial",
        "cordial-run",
        "stud",
        "stud-runtime-bi",
        "stud-render-hos",
        "tipsy",
        "tipsy-gui",
        "mocktail",
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

/// The client settings ("FastFlags") JSON for `nativeInitClientSettings`:
/// `FELINE_SETTINGS` (the JSON itself), else the file `FELINE_SETTINGS_FILE`
/// or `<runtime>/ClientSettings.json` (Roblox's published settings for the
/// Android app, as the app downloads them), else an empty set, which the
/// engine accepts and fills with its built-in defaults.
fn client_settings(runtime: &std::path::Path) -> String {
    if let Ok(json) = std::env::var("FELINE_SETTINGS") {
        return json;
    }
    let file = std::env::var_os("FELINE_SETTINGS_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| runtime.join("ClientSettings.json"));
    match std::fs::read_to_string(&file) {
        Ok(json) => {
            eprintln!(
                "feline-runtime: client settings from {} ({} KB)",
                file.display(),
                json.len() / 1024
            );
            json
        }
        Err(_) => {
            eprintln!("feline-runtime: no client settings file, using the engine's defaults");
            r#"{"applicationSettings":{}}"#.to_owned()
        }
    }
}

/// Feline's runtime data folder: `$XDG_DATA_HOME/feline/runtime` (or
/// `~/.local/share/feline/runtime`).
/// `--play`: the window, Roblox's app, and no time limit, without the
/// developer variables (FELINE_START, FELINE_WINDOW, FELINE_APP).
static PLAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The logged-in user's ID, once the saved session was checked.
static USER_ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// "No time limit": a year, as a number of seconds.
const UNTIL_CLOSED: u64 = 365 * 24 * 60 * 60;

fn playing() -> bool {
    PLAY.load(std::sync::atomic::Ordering::Relaxed)
}

/// A start-up step that `--play` always does and `--init` does only when
/// its developer variable is set.
fn wanted(variable: &str) -> bool {
    playing() || std::env::var_os(variable).is_some()
}

/// Gives the saved login (see login.rs) to the engine, in two steps:
///
/// - `"early"` (before the app starts): asks Roblox whose session it is,
///   tells the engine (`NativeSettingsInterface.nativeSetUserId`: without
///   it the app shows its logged-out screen even with a valid session) and
///   answers the app's account questions with it (NativeUserJavaInterface's
///   `getUserId`, `getUsername`, `getDisplayName`; SessionReporter's last
///   logged-in user). An expired session is forgotten.
/// - `"started"` (right after StartApp): the cookie, as the Android app's
///   Java code hands it over. Earlier, the engine has no cookie store yet
///   and silently drops it.
///
/// FELINE_COOKIE_CHECK=1 reports the cookie names the engine then holds.
fn session_step(
    elf: &feline_runtime::elf::Elf,
    image: &feline_runtime::load::Image,
    stage: &str,
) -> Result<(), String> {
    use std::sync::Mutex;
    const SETTINGS: &str = "com/roblox/engine/jni/NativeSettingsInterface";
    const USER: &str = "com/roblox/engine/jni/user/NativeUserJavaInterface";
    const REPORTER: &str = "com/roblox/engine/jni/reporter/SessionReporterJavaInterface";
    // The cookie of a session Roblox accepted, from "early" to "started".
    static ACCEPTED: Mutex<Option<String>> = Mutex::new(None);
    match stage {
        "early" => {
            let data = runtime_data()?;
            let Some(cookie) = feline_runtime::login::saved_cookie(&data) else {
                return Ok(());
            };
            match feline_runtime::login::account(&cookie) {
                Ok(account) => {
                    use feline_runtime::jni::{set_known_number, set_known_string};
                    set_known_number(USER, "getUserId", account.id as f64);
                    let _ = USER_ID.set(account.id);
                    set_known_string(USER, "getUsername", &account.name);
                    set_known_string(USER, "getDisplayName", &account.display_name);
                    set_known_string(REPORTER, "getLastLoggedInUserId", &account.id.to_string());
                    set_known_string(REPORTER, "getLastLoggedInUser", &account.name);
                    // The engine's own record of the current user.
                    feline_runtime::init::call_java_native(
                        elf,
                        image,
                        SETTINGS,
                        "nativeSetUserId",
                        true,
                        &[feline_runtime::jni::new_string(&account.id.to_string()) as usize],
                    )?;
                    eprintln!("feline-runtime: logged in as {}", account.name);
                    feline_runtime::versions::logged_in();
                }
                Err(feline_runtime::login::AccountError::Expired) => {
                    feline_runtime::login::forget_cookie(&data);
                    eprintln!(
                        "feline-runtime: the saved login has expired; sign in again"
                    );
                    return Ok(());
                }
                // Offline or similar: the session may still be fine.
                Err(feline_runtime::login::AccountError::Other(e)) => {
                    eprintln!("feline-runtime: couldn't check the saved login: {e}");
                }
            }
            *ACCEPTED.lock().unwrap_or_else(|p| p.into_inner()) = Some(cookie);
        }
        // Just before the app starts, and again once it has: newer Roblox
        // versions (2.746) ask for the logged-in user within about a
        // second of starting, before the "started" handover arrived, and
        // logged out ("Authentication token is missing").
        "starting" => {
            let cookie = ACCEPTED.lock().unwrap_or_else(|p| p.into_inner()).clone();
            if let Some(cookie) = cookie {
                feline_runtime::init::call_java_native(
                    elf,
                    image,
                    SETTINGS,
                    "nativeSetMultipleCookies",
                    true,
                    &[
                        feline_runtime::jni::new_string("https://www.roblox.com/") as usize,
                        feline_runtime::jni::new_string(&cookie) as usize,
                    ],
                )?;
                eprintln!("feline-runtime: gave the saved session to the app (before it starts)");
            }
        }
        "started" => {
            let cookie = ACCEPTED.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(cookie) = cookie {
                feline_runtime::init::call_java_native(
                    elf,
                    image,
                    SETTINGS,
                    "nativeSetMultipleCookies",
                    true,
                    &[
                        feline_runtime::jni::new_string("https://www.roblox.com/") as usize,
                        feline_runtime::jni::new_string(&cookie) as usize,
                    ],
                )?;
                eprintln!("feline-runtime: gave the saved session to the app");
            }
            if std::env::var_os("FELINE_COOKIE_CHECK").is_some() {
                let text = feline_runtime::init::call_java_native(
                    elf,
                    image,
                    SETTINGS,
                    "nativeGetCookiesForDomain",
                    true,
                    &[feline_runtime::jni::new_string("https://apis.roblox.com/") as usize],
                )
                .ok()
                .and_then(|h| feline_runtime::jni::string_value(h as _))
                .unwrap_or_default();
                // Names only: values are never shown.
                let names: Vec<&str> = text
                    .lines()
                    .filter_map(|line| line.split('\t').nth(5))
                    .collect();
                eprintln!("feline-runtime: the app's cookies for Roblox: {names:?}");
            }
        }
        _ => {}
    }
    Ok(())
}

/// Marks Roblox's saved client settings (`flag_cache.dat`, and the
/// `tombstone.dat` next to it) as just written. The app treats a cache
/// older than about a minute as expired (the tombstone holds "expiry 1"),
/// expecting Android's Java side to fetch new settings; with no Java side
/// it reported onFlagsFailed, then ignored surface changes
/// ("Flags-Not-Received"), ran near 60 FPS and left parts of Home empty.
/// Every start within a minute of the previous one loaded fine. The
/// settings themselves are refreshed by the engine at each start.
fn freshen_flag_cache(dir: &std::path::Path) {
    let now = std::time::SystemTime::now();
    for name in ["flag_cache.dat", "tombstone.dat"] {
        let path = dir.join(name);
        let result = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|file| file.set_modified(now));
        match result {
            Ok(()) => eprintln!("feline-runtime: {name} marked fresh"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("feline-runtime: couldn't freshen {}: {e}", path.display()),
        }
    }
}

fn runtime_data() -> Result<std::path::PathBuf, String> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .ok_or("no home folder")?;
    Ok(base.join("feline/runtime"))
}

/// The device the app runs on, described honestly, for the app's
/// `DeviceStaticParams` (read during start-up): Android 13, as the system
/// properties say, made by "Feline", model "Linux", and the app's own
/// version from its manifest. Empty answers made the app think it ran on
/// a very old Android ("update your device").
fn describe_device() {
    let app_version = feline_runtime::assets::apk_bytes()
        .and_then(feline_runtime::manifest::apk_version_name)
        .unwrap_or_default();
    eprintln!("feline-runtime: app version {app_version}");
    // The user agent the app's HTTP requests and scripts see, in the
    // Roblox Android app's format, with the same honest device details.
    let user_agent = format!(
        "Mozilla/5.0 (Linux; Android 13; Linux) Roblox/Android RobloxApp/{app_version} (GlobalDist; AndroidApp)"
    );
    feline_runtime::jni::set_known_string(
        "com/roblox/engine/jni/autovalue/InitParams",
        "userAgent",
        &user_agent,
    );
    for (field, value) in [
        // The API level, not the version name: the engine reads it as a
        // number (strtol) and compares it with API levels, e.g.
        // DFStringAndroidDeprecationMinUpgrade = 26 and
        // FIntGraphicsVulkanMinAndroidVersion = 25. "13" read as API 13
        // gave "Roblox no longer supports Android 6 and 7" and no Vulkan.
        ("osVersion", "33"),
        ("manufacturer", "Feline"),
        ("deviceName", "Linux"),
        ("deviceSku", "feline"),
        ("socModel", "x86_64"),
        ("appBuildVariant", "release"),
        ("appVersion", app_version.as_str()),
    ] {
        // The same details twice: DeviceStaticParams (read when the app
        // starts) and DeviceParams (nativeSetDeviceInfo).
        for class in [
            "com/roblox/engine/jni/model/DeviceStaticParams",
            "com/roblox/engine/jni/model/DeviceParams",
        ] {
            feline_runtime::jni::set_known_string(class, field, value);
        }
    }
    for (field, value) in [
        ("country", "US"),
        ("displayResolution", "1920x1080"),
        ("networkType", "WIFI"),
    ] {
        feline_runtime::jni::set_known_string(
            "com/roblox/engine/jni/model/DeviceParams",
            field,
            value,
        );
    }
}
