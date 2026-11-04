# M2: Runtime research

What Feline's own runtime has to provide to run Roblox's Android client on
Linux, how it will be built, and what is still open. No runtime code exists
yet; this document is the plan for M3.

Tags: **[V]** verified (with the command or source), **[A]** reasonable
assumption, **[H]** hypothesis to test in M3.

## 1. Decisions

| Decision | Choice | Why |
|---|---|---|
| Language | **Rust** (stable), `unsafe` only at the C boundary, `asm!` where needed. C only if reusing an existing, license-checked component saves a large amount of work. | Most runtime bugs are memory bugs in parsers and shims; Rust prevents them. FPS doesn't depend on the runtime's language: almost all time per frame is spent in Roblox's own compiled engine and the GPU driver. |
| C variadic functions (`printf(fmt, ...)` style, needed for bionic and `liblog`) | Defined in Rust | **[V]** stable rustc 1.99.0 compiles `unsafe extern "C" fn f(fmt: *const c_char, mut args: ...)`. The runtime crate sets `rust-version = "1.99"` until an older minimum is verified. |
| Java side | Feline's own minimal "Java world" in Rust (option B in §4) | Running Roblox's Java code needs a full ART and libcore, which is heavy and has GPL-2.0 licensing issues. |
| Graphics | In the same process, straight to the host driver | Forwarding every GL/Vulkan call to another process costs time on every call (§6). |
| Clean room | No code from Sober, Cordial, Stud or other Roblox runtimes is read or used (see CONTRIBUTING.md) | Independence and licensing. |

## 2. What the engine needs

Source: Roblox Android client 2.742.0.1059, x86-64 (`libroblox.so`, 120.6 MB,
the copy Cordial had downloaded). Only the **interface** was read with
`readelf` (needed libraries, imported/exported symbol names, ELF headers);
none of Roblox's code was disassembled.

### Needed libraries [V] (`readelf -d`)

`libandroid.so`, `liblog.so`, `libOpenMAXAL.so`, `libmediandk.so`,
`libGLESv2.so`, `libEGL.so`, `libOpenSLES.so`, `libm.so`, `libdl.so`,
`libc.so`. Flags `BIND_NOW`: every import is resolved at load time.

### Imported functions: 576 [V] (`readelf --dyn-syms`, undefined)

| Provider | Count | Examples | Plan |
|---|---|---|---|
| bionic libc | 353 | `pthread_*`, `__FD_SET_chk`, `__cxa_atexit`, `syslog` | Mostly forward to glibc; translate where bionic's ABI differs (§5) |
| libGLESv2 | 74 | `glBindBuffer`, `glAttachShader` | Forward to the host's GLES (via EGL) |
| libm | 55 | `sin`, `atan2f`, `pow` | Forward to glibc's libm |
| libandroid (NDK) | 27 | `AAssetManager_open`, `ANativeWindow_*`, `AInputQueue_*`, `ALooper_*` | Implement (assets from the APK, window → Wayland/X11) |
| libmediandk | 23 + 10 data symbols | `AMediaCodec_*`, `AMEDIAFORMAT_KEY_*` | Stub first (video playback); later via host decoders |
| libEGL | 17 | `eglCreateWindowSurface` | Forward to the host EGL, map `ANativeWindow` to a host surface |
| libdl | 6 | `dlopen`, `dlsym`, `dl_iterate_phdr` | Implement in the loader (the engine opens Vulkan and audio itself) |
| liblog | 4 | `__android_log_print` | Implement: write to Feline's session log |
| Weak, optional | 8 | `ZSTD_trace_*`, `__gcov_*` | Leave unresolved (weak) |

This matches Cordial's published count of 91 EGL/GL symbols (17 + 74) [V].

**Not imported** [V]: any Vulkan function, and any OpenSL ES / OpenMAX AL
function, although both audio libraries are listed as needed. **[H]** The
engine `dlopen`s Vulkan (Cordial's docs say so) and its audio backend; M3's
`dlopen` log will show which.

### Exported entry points [V]

- **557 `Java_*` functions**, e.g. 82 in `…engine.jni.NativeGLInterface`,
  33 in `NativeInputInterface`, 32 in `NativeSettingsInterface`, plus
  account, linking, message bus and web view groups. On Android, Roblox's
  Java app code calls these.
- **`JNI_OnLoad` exists** [V] (exported as `JNI_OnLoad@@LIBROBLOX`). An
  earlier `readelf` filter in this research missed it because of the version
  suffix; `feline-runtime --check` found it. It's the first function an
  Android app calls after loading the library, so the runtime calls it first
  (it may also register natives with `RegisterNatives`).

### ELF details that shape the loader [V]

- 3 `LOAD` segments, `GNU_RELRO`, no `TLS` segment: no ELF thread-local
  storage. Thread-local data uses `pthread_key_create/getspecific`, which can
  be forwarded.
- Relocations: `.rela.plt` (545 `R_X86_64_JUMP_SLOT`) and `.rela.dyn` in
  **Android's packed format (`ANDROID_RELA`, "APS2")**, 2 MB. Decoded by
  `feline-runtime --check` [V]: **572,759 relocations** in total, of four types
  only: 572,132 `R_X86_64_RELATIVE`, 545 `JUMP_SLOT`, 60 `GLOB_DAT`, 22
  `R_X86_64_64`. All of them land inside the library's memory image, have a
  known type and name an existing symbol.
- Segments: `r-x` 115.0 MB, `rw-` 5.2 MB, `rw-` 0.4 MB in the file / 8.1 MB in
  memory (zero-filled data). Symbols: 1,137 (576 imported, 561 exported).
- `INIT_ARRAY` of 29,424 bytes (3,678 constructors) runs at load.

## 3. Architecture

```
feline (launcher) ──Feline runtime protocol (Unix socket, JSON lines)──► feline-runtime (one process per game)
                                                                        ├─ loader    map libroblox.so, decode APS2, relocate, run INIT_ARRAY
                                                                        ├─ bionic    the 353 libc imports: forward / translate / implement
                                                                        ├─ jni       JavaVM + JNIEnv function tables, object handles, fake classes
                                                                        ├─ android   AAsset (APK zip), ANativeWindow, input, looper, config
                                                                        ├─ gfx       EGL/GLES forward; Vulkan: host loader + android-surface translation
                                                                        ├─ audio     whatever the engine dlopens (M3 finds out)
                                                                        └─ media     AMediaCodec stubs, later host decoding
```

- **One crate, `feline-runtime`, with modules**, split into crates only when
  a module gets big enough to need its own tests.
- **The launcher never loads the engine.** If the runtime crashes, the
  launcher shows why; if the launcher closes, the game keeps running.
- **Protocol:** Feline's own, versioned; it replaces reading the Roblox log
  for Discord and monitoring. Events: `started`, `joined {place_id,
  universe_id}`, `left`, `exiting {reason}`; request: `stop`. Designed with
  the M3 code, not before.

## 4. The Java side: the biggest piece

The engine expects to live inside an Android app: Java code calls its 557
natives, and it calls back into Java (`FindClass`, `GetMethodID`,
`CallVoidMethod`, …) through the `JNIEnv` table.

| Option | How | Verdict |
|---|---|---|
| A. Run Roblox's Java code | ART (e.g. `art_standalone`) + AOSP libcore + the APK's dex code | Heavy. Parts of libcore are GPL-2.0 with Classpath exception, which needs review. Rejected for now. |
| **B. Feline's own Java world** | Rust implements `JavaVM`/`JNIEnv`, fake classes and methods for exactly the callbacks the engine makes, and calls the natives in the order an Android app would | **Chosen.** Small, fast, testable. |

**How to find out what's needed without reading Roblox's code:** in M3, every
`FindClass`/`GetMethodID`/`GetStaticMethodID` the engine makes is logged and
answered with a stub that reports "not implemented". The log is the to-do list.
The same "stub and log" rule applies to every unimplemented import: **report
failure, never pretend success**, so gaps stay visible.

## 5. bionic vs glibc: where forwarding isn't enough [A]

Both are C libraries for Linux, so most functions can be forwarded as-is.
Known differences to handle (to be checked against AOSP's Apache/BSD-licensed
headers):

- `errno`: bionic's `__errno()` vs glibc's `__errno_location()`.
- `stdio`: bionic's `FILE` and `__sF` (stdin/stdout/stderr) differ.
- `pthread_*` types: bionic's `pthread_mutex_t` etc. have different sizes.
  Each must be backed by a glibc object, not reinterpreted in place.
- `struct stat`, `dirent`, `sigaction`, `epoll_event` layouts.
- `_chk` fortify functions (`__FD_SET_chk`, …): implement directly.
- Stack protector: both read the guard from `%fs:0x28` on x86-64 [A], so
  the engine runs on glibc's threads unchanged.

## 6. Performance design

| Decision | Reason |
|---|---|
| Graphics in-process, direct to the driver | No per-call IPC. (Stud forwards GL/Vulkan over a socket to a separate process, which isolates crashes but adds a cost to every call.) |
| Frame limiting at presentation (mailbox / timed present) | Observed: an engine-internal limiter vs Cordial's present-time limiter is a plausible frame-pacing difference. To be measured. |
| Allocator | Forward `malloc` to glibc first; try mimalloc only with measurements. |
| Thread priorities | Render and audio threads may need higher priority; measure first. |
| No per-frame IPC | Monitoring through `/proc` and a future shared-memory counter. |

Every performance claim needs a reproducible benchmark (frame times with
MangoHud, same place, same settings, ≥5 runs), compared with Sober and Cordial.

## 7. Legal

- **Interface inspection only.** Reading the library's names and headers is
  what `ldd`/`readelf` do and is needed for compatibility. No disassembly or
  decompilation of Roblox's code. Feline ships no Roblox files.
- **Account risk** is the same as for the other runtimes (see LEGAL.md), and
  possibly higher for a new runtime. Test with a throwaway account.
- **License of the runtime: GPL-3.0-or-later with an additional permission
  (GPL-3.0 section 7)** allowing the runtime to load and run the proprietary
  Roblox client the user obtained. Without it, running a proprietary program
  inside a GPL program's process is legally unclear. Stud uses the same
  approach (AGPL-3.0 with a section 7 permission). Draft:

  > Additional permission under GNU GPL version 3 section 7: If you modify
  > this Program, or any covered work, by linking or combining it with the
  > Roblox client (or a modified version of that client), containing parts
  > covered by the terms of Roblox Corporation's license, the licensors of
  > this Program grant you additional permission to convey the resulting work.

  This applies to code the Feline project owns. **Have it reviewed before the
  first release**; this document is not legal advice.

## 8. M3 plan: "loaded and imports resolved"

| Step | Done when |
|---|---|
| 1. `feline-runtime` crate; command `feline-runtime --check <libroblox.so>` | Builds; prints the ELF summary above |
| 2. Map segments, decode APS2, apply relocations | Unit tests on small test libraries; the engine maps without errors |
| 3. Symbol table: every import → implemented, forwarded or a logging stub | Report: 576 / 576 resolved, listed per category |
| 4. Run `INIT_ARRAY` | All constructors return, with every stub hit logged |
| 5. Minimal `JavaVM`/`JNIEnv`, call `JNI_OnLoad`, then a first native, with logging | They return; the log lists the Java callbacks they wanted |

**Progress:**
- Step 1 done: `feline-runtime --check` reads the engine with a
  bounds-checked ELF reader and an APS2 decoder and validates every
  relocation (see §2).
- Step 2 done: `feline-runtime --load` maps the engine (128.4 MB), applies
  572,132 `RELATIVE` relocations and 1 to its own symbol, and sets W^X
  permissions plus 5.2 MB of read-only RELRO, in 0.3 s. The remaining 626
  relocations need the 576 imports (step 3). `unsafe` is limited to four
  operations in `load.rs`; all writes go through a bounds-checked slice.
- Step 3 done: `feline-runtime --load` binds **all 576 imports** and all 626
  import relocations: 195 forwarded to glibc, 63 to glibc's libm, 74 to the
  system's OpenGL ES, 8 glibc variables, 15 Feline-owned variables, 6
  optional left empty, and **215 stubs** (44 `pthread_*`, 23 stdio, 23 media,
  17 EGL, 16 bionic internals and fortify functions, ~20 locale/wide-char,
  and Android's config, looper, asset, window and `dl*` APIs). Forwarding is
  limited to functions whose whole binary interface matches; known
  mismatches (`addrinfo` field order, 8- vs 128-byte `sigset_t`, 128- vs
  80-bit `long double`, `_SC_*` numbers, `__assert` argument order, …) are
  stubs until Feline implements them. Each stub is generated code that
  names the missing function and stops (`--imports` lists them all).
- Step 4 done: `feline-runtime --init` runs the engine's constructors (its
  first code to run), without network (`unshare -rn`). **All 3,678 ran and
  returned.** Getting there, in the order the engine needed them: pthreads
  (forwarded: same object sizes and all-zero initializers on x86-64,
  measured with gcc), locales and multibyte text (opaque `locale_t`, same
  `LC_*` numbers, 8-byte `mbstate_t` in both), `sysconf` (Feline's: 145 of
  146 `_SC_*` numbers differ, table generated from AOSP's header and
  glibc's), stdio (opaque `FILE*`, so all forwarded, `stdin`/`stdout`/
  `stderr` are glibc's), bionic's own fortify functions (Feline's), and the
  `dl*` family (Feline's: `dl_iterate_phdr` reports the engine for C++
  unwinding; `dlopen` logs requests, so far only `libc.so`). The engine
  starts its own threads during the constructors.
- Step 5 in progress: Feline's Java world (`jni.rs`: JavaVM 8 slots, JNIEnv
  233 slots, order generated from AOSP's `jni.h`) answers `JNI_OnLoad`. It
  looks up classes such as `com/roblox/engine/jni/locale/
  NativeLocaleJavaInterface` and `…/user/NativeUserJavaInterface` and their
  static getters, then the main thread **overflows its stack in infinite
  recursion inside the engine** (the same three return addresses repeating).
  Ruled out, by observation only: the constructors alone (no crash without
  `JNI_OnLoad`), stack size (also with a 512 MB stack), signal handlers
  (the engine installs none: `SigCgt`), other threads (its two workers are
  idle on a futex). How far `JNI_OnLoad` gets before the loop varies between
  runs. Diagnostics: `FELINE_JNI_TRACE=1` (every JNI call),
  `FELINE_THREAD_WATCH=1` (thread snapshot in crash reports),
  `FELINE_SKIP_ONLOAD=1`; crash reports give the engine offset, the faulting
  address and the engine return addresses on the stack (on an alternate
  signal stack, so stack overflows are reported too).

**Not a goal of M3:** a window, a game, or any claim that Roblox runs.

## 9. Risks

1. Roblox policy (kicks or bans for unsupported clients).
2. Roblox updates weekly; imports and Java callbacks will change. Mitigation:
   the stub-and-log design makes changes visible immediately.
3. The Java side may turn out much larger than expected (557 natives).
4. Anti-tamper or attestation checks in the client [H]; Feline won't try to
   defeat them.
5. Effort: this is months of work. M3 shows the real size early.
