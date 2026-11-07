# Legal notes

Feline is an independent, unofficial project. It is **not affiliated with,
endorsed by or supported by Roblox Corporation**. "Roblox" is a trademark of
Roblox Corporation and is used here only to describe what Feline works with.

## What Feline does and doesn't do

- Feline **does not ship any Roblox files**: no APKs, no engine libraries, no assets.
- Feline's own runtime **does not exist yet**. Today Feline can optionally
  start an **external third-party runtime** that you installed yourself
  (currently supported: [Sober](https://sober.vinegarhq.org),
  [Cordial](https://github.com/luohoa97/cordial) and [Stud](https://github.com/CatPieLeaf/Stud)),
  through that program's public command line. Feline never selects one for you.
- Feline **never handles your Roblox login**. Sign-in happens inside the runtime.
- Feline **does not modify, inject into or inspect** the Roblox client or its
  network traffic.

## Account risk: please read

Roblox does not support third-party clients on Linux.

- In August 2025 Roblox announced that players without edit permissions on an
  experience are kicked if detected on an emulator
  ([DevForum announcement](https://devforum.roblox.com/t/an-update-on-using-third-party-emulators/3867040)).
  Whether Android-runtime projects count as emulators is not officially stated.
- Cordial's README states that Roblox bans accounts for using third-party clients.

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

Feline's future runtime will run the proprietary Roblox client, obtained
separately by the user, in the same process. **Chosen approach (M2):** the
runtime is GPL-3.0-or-later with an **additional permission under GPL-3.0
section 7** allowing it to be combined with the Roblox client. The draft text is
in [runtime/M2-RESEARCH.md](runtime/M2-RESEARCH.md#7-legal). It will be added as
`LICENSE.exception` together with the first runtime code (M3), and should be
reviewed before the first release. Not legal advice.

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
| [Slint](https://slint.dev) 1.18 (GUI) | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | Used under **GPL-3.0-only**. Feline's *source* stays GPL-3.0-or-later; *binaries* that include Slint are effectively GPL-3.0-only |
| Cordial (optional) | GPL-3.0-or-later | **Not a dependency.** A separate program the user installs, started through its public command line. Feline contains none of its code |
| Sober (optional) | Proprietary (closed source) | **Not a dependency.** Only an already-installed Flatpak is started; Feline never downloads or redistributes it |
| Stud (optional) | AGPL-3.0 (with a section 7 permission) | **Not a dependency.** A separate program the user installs, started through its command line. Feline never downloads or bundles it |
| Tipsy (optional) | GPL-3.0-or-later | **Not a dependency.** A separate program the user installs, started through its command line. Feline contains none of its code |
| Mocktail, Nightcap (optional) | Apache-2.0 | **Not a dependency.** Separate programs the user installs, started through their command line. Feline's launcher contains none of their code |

## Runtime component policy

Feline's runtime is developed independently.

- **Never used:** code from Cordial, Sober or Stud, copied, ported, translated or
  adapted. This is project policy (see [CONTRIBUTING.md](../CONTRIBUTING.md)),
  even where a license would allow it.
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
