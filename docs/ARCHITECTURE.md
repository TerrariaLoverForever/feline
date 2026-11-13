# Architecture

## Overview

```
┌──────────────────────┐   Feline runtime protocol   ┌───────────────────────────┐
│ feline (launcher)    │ ──────────────────────────► │ feline-runtime (Linux)    │
│ GUI + text commands  │   separate process          │ own Android compatibility │
└──────────┬───────────┘                             │ layer, independently      │
           │ uses                                    │ developed                 │
┌──────────▼───────────┐                             └───────────────────────────┘
│ feline-core          │                             ┌───────────────────────────┐
│ system info          │ ── optional, opt-in ──────► │ external runtimes         │
│ RuntimeBackend trait │    public CLI only          │ (e.g. Cordial): never     │
│ process handling     │                             │ required, never default   │
└──────────────────────┘                             └───────────────────────────┘
```

All Feline code is licensed GPL-3.0-or-later.

## Rules

1. **Feline will have its own runtime, developed independently.** It is not a
   fork, port, translation or adaptation of Cordial, Sober or any other Roblox
   runtime. See [CONTRIBUTING.md](../CONTRIBUTING.md) for the clean-room rule.
2. **The launcher and the runtime are separate programs.** The launcher
   starts `feline-runtime --play <engine>` and hands it its settings in
   `~/.config/feline/runtime.json`; a protocol of Feline's own between the
   two (versioned, with no dependency on any other project's protocol) may
   follow.
3. **External runtimes are optional adapters.** They are used only through
   their public interfaces, are never required, are never selected
   automatically, and are always labelled "external third-party runtime".
4. **`RuntimeBackend` is written in Feline's own terms** (detect, version,
   doctor, diagnostics, launch), so `feline-runtime` is just another backend
   (`runtime::feline`).
5. **The launcher is never in the game's critical path.** A runtime runs in its
   own process group, so closing or crashing the launcher doesn't end the game.
6. **The GUI never blocks.** Slow work runs on a background thread.
7. **Small dependency set.** Third-party crates: `slint` (GUI, default
   features off), `serde_json` (JSON settings files, Discord and Roblox API
   messages) and `ureq` with `rustls` (HTTPS for Roblox's public web API).
   Every dependency is license-checked first (see [LEGAL.md](LEGAL.md)).
8. **Editing a runtime's own settings file is conservative.** Only settings
   whose values were verified are listed; files are never created, never
   written while the runtime runs or when they can't be parsed, and unknown
   keys, key order, indentation and comment headers are kept
   (`runtime::config`).

## `feline-core` modules

| Module | Purpose |
|---|---|
| `system` | Host info from `/proc`, `/sys`, `/etc/os-release`, `pci.ids`; quick checks (`SystemInfo::checks`) for the Diagnostics page |
| `link` | Checks pasted game links; converts `roblox.com/games/<id>` to a `roblox://` link |
| `process` | Runs helper commands with captured output, an output cap and a timeout |
| `runtime` | `RuntimeBackend` trait, `Origin` (Feline vs external), `Installation`, `detect_all` |
| `settings` | `~/.config/feline/settings.conf`: chosen runtime, accepted account-notice version |
| `runtime::config` | Settings lists (`SettingDef`), safe editing of a runtime's JSON file (`ConfigFile`), Feline's overrides (`managed_value`), "is it running?" |
| `monitor` | CPU and memory of the running game: every descendant of the started process, from `/proc` (CPU time from `stat`, memory as PSS from `smaps_rollup`). No FPS: that needs a graphics layer |
| `presence` | Feline's Discord status: `log` (follows the Roblox client log), `roblox` (public web API lookups), `discord` (Discord's local socket protocol), `Session` (background thread per game), `service` (`feline --presence`: a helper process started next to each game, lives until the game ends, single instance through a lock file) |
| `runtime::sober` | Optional adapter for the external Sober runtime (Flatpak only, closed source): `sober [link]` |
| `runtime::stud` | Optional adapter for the external Stud runtime: Flatpak / `stud` on `PATH` (verified by its metainfo) / AppImage, `stud [link]` |
| `runtime::tipsy` | Optional adapter for the external Tipsy runtime: Flatpak / `tipsy-gui` on `PATH` / AppImage, `tipsy-gui --play [link]`, `tipsy version` |
| `runtime::mocktail` | Optional adapters for the external Mocktail and Nightcap runtimes (one code path, two backends): Flatpak / `mocktail` on `PATH` (told apart by the metainfo next to it) / AppImage, `mocktail [link]` |
| `runtime::cordial` | Optional adapter for the external Cordial runtime: finds Flatpak / `PATH` / AppImage installs, uses `--help`, `--doctor`, `--diagnostics` |

Detection never starts processes; only `Installation::version()` (when the
version isn't in a file), `doctor()` and `diagnostics()` do, always with a timeout.

## `feline-runtime`

Feline's own runtime, licensed GPL-3.0-or-later with the additional
permission in [`LICENSE.exception`](../LICENSE.exception). See
[runtime/M2-RESEARCH.md](runtime/M2-RESEARCH.md) for the research it started
from. The table lists the parts it was first built from; newer ones
(`vulkan`, `egl`, `audio`, `voice`, `text`, `keys`, `gamepad`, `login`,
`prefs`, `pacing`, `tuning`, `net`, `versions`) describe themselves at the
top of their files in `crates/feline-runtime/src/`.

| Module | Purpose |
|---|---|
| `elf` | Bounds-checked ELF64 reader: segments, dynamic section, symbols with versions (GNU hash count), relocation tables |
| `packed` | Decoder for Android's packed relocations (`APS2`, SLEB128) |
| `check` | `feline-runtime --check <lib>`: summary and consistency check of every relocation |
| `load` | `feline-runtime --load <lib>`: maps the segments, applies all relocations (imports from `imports`), sets permissions (W^X, RELRO) |
| `imports` | Binds each import: forwarded to glibc / OpenGL ES (only where the binary interface matches), Feline-owned variables, stubs, or weak-empty. `--imports` lists them |
| `host` | Opens `libc.so.6`, `libm.so.6`, `libGLESv2.so.2` by name and looks symbols up there |
| `stubs` | Generated x86-64 stubs: each names the missing function (and the calling thread) and stops the process |
| `bionic` | Feline's implementations where bionic and glibc differ: `sysconf` (generated `_SC_*` table in `sysconf_table`), bionic-only fortify functions |
| `dl` | `dlopen`/`dlsym`/`dlclose`/`dlerror`/`dladdr`/`dl_iterate_phdr` for the engine |
| `init` | `feline-runtime --init <lib>`: runs the constructors and `JNI_OnLoad`; crash reports (alternate signal stack) give the engine offset, faulting address, engine return addresses, recent import calls and a thread snapshot |
| `jni`, `jni_names` | Feline's Java world: JavaVM (8 slots) and JNIEnv (233 slots, order generated from AOSP's `jni.h`); a JNIEnv per thread with Android-like attach state; classes, member IDs, strings, logged placeholder answers for the 90 `Call*Method` functions |
| `looper` | Android's `ALooper` on epoll; the loading thread gets one automatically, like Android's main thread |
| `assets` | `AAssetManager`/`AAsset` over the APK's `assets/` (mapped zip, stored or deflated entries) and `AConfiguration` (desktop screen, language from `LANG`) |
| `window` | A desktop window (winit, Wayland or X11) behind `ANativeWindow`; its events drive `onSurfaceCreated`/`onSurfaceChanged`/`onWindowFocusChanged`. Test windows are refused while a Roblox runtime runs |
| `liblog` | Android's `__android_log_*`: the engine's own log lines, formatted with glibc's `vsnprintf` |
| `trace` | `FELINE_CALL_TRACE=1`: recording trampolines in front of every bound import function |

`unsafe` is allowed only in `load`, `host` and `stubs`, each use with a
`SAFETY` comment.

## Toolchain

Feline builds with a distribution-packaged Rust toolchain; rustup is not
required. The minimum supported version is `rust-version` in `Cargo.toml`.
