# Legal notes

Feline is an independent, unofficial project. It is **not affiliated with,
endorsed by or supported by Roblox Corporation**. "Roblox" is a trademark of
Roblox Corporation and is used here only to describe what Feline works with.

## What Feline does and doesn't do

- Feline **does not ship any Roblox files**: no APKs, no engine libraries, no assets.
- **Automatic Roblox updates (Feline's own runtime only):** when Feline's
  runtime is the chosen runtime, the launcher downloads Roblox's official
  Android app (`com.roblox.client`, x86_64) onto your computer from APKPure,
  as Mocktail does, for your own use. Nothing is accepted unless it passes
  Android's APK Signature Scheme v2/v3 check **and** is signed with Roblox's
  own certificate (SHA-256 `44932ea3…ba1477`, pinned in
  `feline-core/src/engine/mod.rs`). Feline never modifies, repackages or
  redistributes the app. `feline --update-roblox` does the same by hand.
- Feline's own runtime (`feline-runtime`) runs that app.
  Feline can also start an **external third-party runtime** that you
  installed yourself (Sober, Cordial, Stud, Tipsy, Mocktail, Nightcap),
  through that program's public command line. Feline never selects one for you.
- **Your Roblox login:** with external runtimes, sign-in happens inside them.
  With Feline's own runtime, you sign in on Roblox's own website in Feline's
  login window; the resulting session cookie is saved only on your computer
  (`~/.local/share/feline/runtime/roblox-cookie`, readable only by you) and
  sent only to Roblox. Feline never sees or stores your password.
- **The official client on Windows, and external runtimes:** Feline injects
  nothing into them and doesn't read or change their network traffic. It
  starts them, writes settings they accept, and (for mods) swaps asset files
  such as sounds, fonts and textures, with a backup.

## What Feline's own runtime does to Roblox's app

Feline's runtime doesn't start Roblox's app from outside: it **loads the
app's engine into its own process** and plays the part of Android for it.
That means more contact with Roblox's code than a launcher has, and this is
all of it:

- **It answers what the engine asks.** Every Android function the engine
  imports (files, network, sound, graphics, the Java side) is Feline's own
  code. The engine's file on disk is never changed.
- **It calls one engine function that the engine doesn't export.** The
  engine frees some buffers with its built-in allocator, so Feline has to
  hand it buffers from that same allocator, and calls the engine's internal
  `malloc` at a fixed address for one known Roblox version
  (`crates/feline-runtime/src/alloc.rs`). Feline checks the bytes at that
  address first and, on any other version, doesn't call it. Finding that
  address meant looking at a small piece of the engine's machine code; see
  "Exceptions" in [CONTRIBUTING.md](../CONTRIBUTING.md).
- **It sets Roblox's own settings:** the frame-rate limit and graphics
  quality in Roblox's settings file, the app's layout policy (desktop
  instead of tablet), and the client flags listed in the launcher.
- **Mods** replace asset files (sounds, fonts, textures, cursors) in the
  runtime's unpacked copy of the app. Code files are never replaced.
- **Network:** Feline doesn't read or change what Roblox sends. Two optional
  settings change *where* it can connect: "Block Roblox tracking" makes
  Roblox's analytics and telemetry servers look unreachable (**off unless
  you turn it on**; see the account-risk section), and "Proxy" sends
  Roblox's web requests through a proxy you name.
- **It reports itself as Android**, since that is the app it runs.

## Account risk: please read

Roblox does not support third-party clients on Linux.

- In August 2025 Roblox announced that players without edit permissions on an
  experience are kicked if detected on an emulator
  ([DevForum announcement](https://devforum.roblox.com/t/an-update-on-using-third-party-emulators/3867040)).
  Whether Android-runtime projects count as emulators is not officially stated.
- Cordial's README states that Roblox bans accounts for using third-party clients.

- Some games kick players on Feline's runtime with "Client out of sync"
  (error 267). Why isn't known yet.
- **"Block Roblox tracking"** (Feline's runtime, off by default) stops the
  app from reaching Roblox's telemetry servers. A client that sends no
  telemetry may stand out. Nobody has measured whether it changes the risk;
  turn it on only if you accept that.

Using Feline does not remove these risks. **Do not use an account you can't
afford to lose.**

## Privacy: what the Discord status sends

Only while the Discord status is switched on and a game Feline started is running:

- **To Roblox's public web API** (no login): the game's universe ID, to get its
  name, creator and icon. Your user ID only if "Show your Roblox username and
  avatar" is on.
- **To the Discord app on your computer** (a local socket, not the internet):
  the status text, images and buttons shown in the Settings page. Discord then
  shows it to people who can see your profile.
- Feline reads the Roblox client's log file for this. It never reads cookies,
  passwords or anything outside that log.

## Feline's license

Feline is free software, licensed under the **GNU General Public License,
version 3 or (at your option) any later version** (`GPL-3.0-or-later`). The
full text is in [`COPYING`](../COPYING). Every source file carries an
`SPDX-License-Identifier: GPL-3.0-or-later` header.

The GPL does not grant rights to the Feline name or logo; see
[TRADEMARKS.md](../TRADEMARKS.md). The icon files in `icon/` and `assets/icons/`
are **not** covered by the GPL: all rights reserved.

### Licensing the runtime: the Roblox client

Feline's runtime runs the proprietary Roblox client, obtained separately by
the user, in the same process. So the runtime (`crates/feline-runtime/`) is
GPL-3.0-or-later with an **additional permission under GPL-3.0 section 7**
that allows combining it with the Roblox client. The text is in
[`LICENSE.exception`](../LICENSE.exception), and every runtime source file
refers to it in its header. How it was chosen is in
[runtime/M2-RESEARCH.md](runtime/M2-RESEARCH.md#7-legal).

The permission was written by the project itself and **has not been reviewed
by a lawyer**. Nothing in this file is legal advice.

## Dependencies

| Component | License | How it's used |
|---|---|---|
| Rust standard library | MIT OR Apache-2.0 | Compiled in. Compatible with GPL-3.0 |
| [serde_json](https://github.com/serde-rs/json) 1 (+ `serde`, `indexmap` for key order) | MIT OR Apache-2.0 | Compiled in. Compatible with GPL-3.0 |
| [ureq](https://github.com/algesten/ureq) 3 with `rustls` (+ `rustls`, `ring`, `webpki-roots`, `http` and small helpers) | MIT / Apache-2.0 / ISC / BSD-3-Clause / Zlib; `ring`: Apache-2.0 AND ISC; `webpki-roots`: CDLA-Permissive-2.0 (certificate data; keep its license text) | Compiled in. All compatible with GPL-3.0. Checked with `cargo metadata` |
| [winit](https://github.com/rust-windowing/winit) 0.30 (runtime: the engine's window; also used by Slint in the launcher) | Apache-2.0 (its Wayland/X11 helpers: MIT / Apache-2.0) | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [miniz_oxide](https://github.com/Frommi/miniz_oxide) 0.9 (runtime only: unpacking deflated APK assets; + `adler2`) | MIT OR Zlib OR Apache-2.0 | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [libc](https://github.com/rust-lang/libc) 0.2 (runtime only: `dlopen`/`dlsym`) | MIT OR Apache-2.0 | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [rustix](https://github.com/bytecodealliance/rustix) 1.1 (runtime only: `mm`, `param`; + `linux-raw-sys`, `bitflags`) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [gilrs](https://gitlab.com/gilrs-project/gilrs) 0.11 (runtime only: controllers; + `gilrs-core`, `vec_map`, `inotify`/`inotify-sys` (ISC), `objc2-io-kit` (macOS only, not built on Linux)) | MIT OR Apache-2.0 (its bundled SDL controller database: zlib) | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [ring](https://github.com/briansmith/ring) 0.17 (launcher: hashing and signature checks of Roblox downloads; already in the build through rustls) | Apache-2.0 AND ISC | Compatible with GPL-3.0 |
| [rsa](https://github.com/RustCrypto/RSA) 0.9 (launcher: RSA checks of Roblox's 1024-bit signing key, below ring's minimum; + `num-bigint-dig`, `sha2`, `pkcs1`, `der` etc.) | MIT OR Apache-2.0 | Compatible with GPL-3.0 |
| [ash](https://github.com/ash-rs/ash) 0.38 (runtime only: Vulkan types and function tables, no loader of its own) | MIT OR Apache-2.0 | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [ab_glyph](https://github.com/alexheretic/ab-glyph) 0.2 (runtime only: drawing typed text) | Apache-2.0 | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [arboard](https://github.com/1Password/arboard) 3 (runtime only: copy and paste) | MIT OR Apache-2.0 | Compiled into `feline-runtime`. Compatible with GPL-3.0 |
| [webkit6](https://gitlab.gnome.org/World/Rust/webkit6-rs) 0.6 and [gtk4](https://github.com/gtk-rs/gtk4-rs) 0.11 (login window only) | MIT (the bindings). WebKitGTK and GTK themselves: LGPL-2.1-or-later (WebKit also BSD parts) | The bindings are compiled into `feline-login`; WebKitGTK and GTK are the system's shared libraries, never bundled |
| [rodio](https://github.com/RustAudio/rodio) 0.23 (launcher: background music; + `cpal` (Apache-2.0), `symphonia` (MPL-2.0)) | MIT OR Apache-2.0 | Compiled in. MPL-2.0 is compatible with GPL-3.0 (its section 3.3) |
| [ksni](https://github.com/iovxw/ksni) 0.3 (launcher, Linux: tray icon; + `zbus` (MIT)) | Unlicense | Compiled in. Compatible with GPL-3.0 |
| [windows-sys](https://github.com/microsoft/windows-rs) 0.61 (Windows only) | MIT OR Apache-2.0 | Compiled in. Compatible with GPL-3.0 |
| [Slint](https://slint.dev) 1.18 (GUI) | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | Used under **GPL-3.0-only**. Feline's *source* stays GPL-3.0-or-later; *binaries* that include Slint are effectively GPL-3.0-only |
| Cordial (optional) | GPL-3.0-or-later | **Not a dependency.** A separate program the user installs, started through its public command line. Feline contains none of its code |
| Sober (optional) | Proprietary (closed source) | **Not a dependency.** Only an already-installed Flatpak is started; Feline never downloads or redistributes it |
| Stud (optional) | AGPL-3.0 (with a section 7 permission) | **Not a dependency.** A separate program the user installs, started through its command line. Feline never downloads or bundles it |
| Tipsy (optional) | GPL-3.0-or-later | **Not a dependency.** A separate program the user installs, started through its command line. Feline contains none of its code |
| Mocktail, Nightcap (optional) | Apache-2.0 | **Not a dependency.** Separate programs the user installs, started through their command line. Their sources were also read as a reference for Feline's runtime; see "Runtime component policy" below |

## Runtime component policy

Feline's runtime is written by the Feline project.

- **Never used:** code from Cordial, Sober or Stud, copied, ported, translated or
  adapted. This is project policy (see [CONTRIBUTING.md](../CONTRIBUTING.md)),
  even where a license would allow it.
- **Used as a reference: Mocktail and Nightcap** (both Apache-2.0). Their
  sources were read to learn how Roblox's app expects to be driven, instead
  of guessing. What Feline takes from them is facts: key, mouse-button and
  gamepad numbers, the order of some of Roblox's input, text, voice and
  message-bus calls, the app policy that gives the desktop layout, which
  of Roblox's settings holds the frame-rate limit, and which values a game
  link carries and how they're handed to the app to join that game. The code is Feline's own;
  no file of theirs is in Feline. Each Feline file that follows them says so
  at the place and names the file it followed (`desktop.rs`, `gamepad.rs`,
  `init.rs`, `join.rs`, `login.rs`, `messagebus.rs`, `overlay.rs`,
  `prefs.rs`, `text.rs`, `voice.rs`, `window.rs` in
  `crates/feline-runtime/src/`).
  Neither project ships a NOTICE file. A copy of their license is in
  [`LICENSES/Apache-2.0.txt`](../LICENSES/Apache-2.0.txt).
  - Mocktail: <https://github.com/komaruworld/mocktail>
  - Nightcap: <https://github.com/CoderDayton/nightcap>
- **Never used:** GPL-2.0-only code, which can't be combined with GPL-3.0.
- **Every third-party component is verified file by file** (license headers
  and LICENSE files, not package metadata) before it is incorporated.

Preliminary research (**unverified**, from package metadata and project
listings; not a decision to use any of these):

| Component | Reported license | Preliminary note |
|---|---|---|
| AOSP bionic, ART, libnativehelper, NDK headers | Apache-2.0 / BSD | Compatible with GPL-3.0 |
| bionic generated kernel headers (`libc/kernel/uapi`) | Derived from Linux; stated to contain no copyrightable material | Needs review before use |
| AOSP libcore (OpenJDK-derived parts) | GPL-2.0 with Classpath Exception | Needs review; avoid if possible |
| libhybris | Apache-2.0 | Compatible |
| mcpelauncher-linker | MIT (unconfirmed) | Compatible if confirmed |
| libjnivm | MIT (unconfirmed) | Compatible if confirmed |
| art_standalone | Apache-2.0 (unconfirmed) | Compatible if confirmed |
| Android Translation Layer | GPL-3.0-or-later | Reference only: another project's runtime |
| Waydroid, Anbox | GPL-3.0 | Not used: different design |
