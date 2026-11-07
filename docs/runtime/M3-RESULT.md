# M3 result: the engine loads and initialises on Feline's runtime

Roblox Android client 2.742.0.1059 (x86-64), `feline-runtime --init`, no network.

| Step | Result |
|---|---|
| 1. Read the engine | 572,759 relocations decoded and checked |
| 2. Map and relocate | 128.4 MB mapped, W^X, RELRO read-only |
| 3. Bind imports | 576/576 (forwarded, Feline's own, stubs) |
| 4. Constructors | all 3,678 ran and returned |
| 5. `JNI_OnLoad` | **returned `JNI_VERSION_1_6`** (reproducible) |

## What fixed `JNI_OnLoad`

Every thread got the same `JNIEnv`, and the engine's main thread then recursed forever (only `pthread_getspecific` in the loop, found with `FELINE_CALL_TRACE=1`). A real Java VM gives every thread its own `JNIEnv`, and threads that never attached get `JNI_EDETACHED` from `GetEnv`. With that behaviour the recursion disappeared. Found by observing the interface only; no engine code was disassembled.

## The Java world the engine asked for (M4 to-do list)

Placeholders today: no Java code runs, so method calls return 0 / false / an empty String / null by signature, and each is logged. The engine copes (it logs warnings such as "DeviceStaticParams is null").

- `com/roblox/engine/jni/locale/NativeLocaleJavaInterface`
  - static `getLocale` `()Ljava/lang/String;`
  - static `getRobloxLocale` `()Ljava/lang/String;`
  - static `getGameLocale` `()Ljava/lang/String;`
- `com/roblox/engine/jni/user/NativeUserJavaInterface`
  - static `getUserId` `()J`
  - static `getIsUnder13` `()Z`
  - static `getUsername` `()Ljava/lang/String;`
  - static `getDisplayName` `()Ljava/lang/String;`
  - static `getAlternateName` `()Ljava/lang/String;`
  - static `getPlatformName` `()Ljava/lang/String;`
  - static `getMembershipType` `()I`
  - static `getHasRobloxSubscription` `()Z`
  - static `getTheme` `()Ljava/lang/String;`
- `com/roblox/universalapp/logging/LoggingProtocol`
  - static `getProcessTimestamp` `()J`
- `com/roblox/engine/jni/NativeGLJavaInterface`
  - static `getDeviceStaticParams` `()Lcom/roblox/engine/jni/model/DeviceStaticParams;`
  - static `showKeyboard` `(JZ[BLcom/roblox/engine/jni/model/NativeTextBoxInfo;)V`
  - static `hideKeyboard` `()V`
  - static `promptNativePurchase` `(JLjava/lang/String;Ljava/lang/String;)V`
  - static `promptNativePurchase` `(JLjava/lang/String;)V`
  - static `promptNativePurchaseWithPayload` `(JLjava/lang/String;Ljava/lang/String;)V`
  - static `promptNativePurchaseWithPaymentSessionId` `(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)V`
  - static `promptNativePurchaseWithPaymentSessionId` `(JLjava/lang/String;Ljava/lang/String;)V`
  - static `exitGameWithError` `(I)V`
  - static `gameDidLeave` `()V`
  - static `onAppShellReloadNeeded` `()V`
  - static `listenToMotionEvents` `(Ljava/lang/String;)V`
  - static `screenOrientationChanged` `(I)V`
  - static `openNativeOverlay` `(Ljava/lang/String;Ljava/lang/String;)V`
  - static `onDataModelNotificationCallback` `(Ljava/lang/String;Ljava/lang/String;)V`
  - static `gameLoadedCallback` `(J)V`
  - static `onLuaTextBoxChangedCallback` `(Ljava/lang/String;)V`
  - static `onLuaTextBoxPropertyChangedCallback` `()V`
  - static `onAppBridgeNotification` `(Ljava/lang/String;Ljava/lang/String;)V`
  - static `onExtendedAnalyticsRecvCallback` `([BI)V`
  - static `saveImageToAlbum` `(Ljava/lang/String;)V`
  - static `onVrSessionStateUpdate` `(I)V`
  - static `getWebViewUserAgent` `()V`
  - static `getMobileAdvertisingId` `()V`
- `com/snapchat/djinni/NativeObjectManager` (looked up only)
- `java/lang/Class`
  - `getClassLoader` `()Ljava/lang/ClassLoader;`
  - `getClassLoader` `()Ljava/lang/ClassLoader;`
- `java/lang/ClassLoader`
  - `loadClass` `(Ljava/lang/String;)Ljava/lang/Class;`
  - `findClass` `(Ljava/lang/String;)Ljava/lang/Class;`
- `com/roblox/engine/jni/reporter/SessionReporterJavaInterface`
  - static `getFilesDir` `()Ljava/lang/String;`
  - static `setEventTrackingGoogleAnalytics` `(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)V`
  - static `getAppVersion` `()Ljava/lang/String;`
  - static `getLastLoggedInUser` `()Ljava/lang/String;`
  - static `getLastLoggedInUserId` `()Ljava/lang/String;`
  - static `sendSessionReport` `(Ljava/lang/String;Ljava/lang/String;)V`
- `com/roblox/audio/AppRtcDeviceWrapper` (looked up only)
- `org/fmod/AudioDevice` (looked up only)
- `org/fmod/MediaCodec` (looked up only)
- `org/fmod/FMOD` (looked up only)
- `com/roblox/engine/jni/video/MediaCodecInfoUtils`
  - static `getVideoCodecs` `()[Lcom/roblox/engine/jni/video/VideoCodecCapability;`
  - static `hevcHardwareEncodingSupported` `(III)Z`
- `com/roblox/engine/jni/util/NetworkUtils`
  - static `getPublicIPv4Addresseses` `()Ljava/lang/String;`
- `com/roblox/engine/jni/memstorage/Connection` (looked up only)
- `com/roblox/universalapp/messagebus/Connection` (looked up only)
- `com/roblox/engine/jni/model/NativeTextBoxInfo` (looked up only)
- `org/webrtc/voiceengine/WebRtcAudioManager` (looked up only)

### Calls answered with placeholders

- `static com/roblox/universalapp/logging/LoggingProtocol.getProcessTimestamp ()J -> 0`
- `static com/roblox/engine/jni/NativeGLJavaInterface.getDeviceStaticParams ()Lcom/roblox/engine/jni/model/DeviceStaticParams; -> null`
- `java/lang/Class.getClassLoader ()Ljava/lang/ClassLoader; -> null`
- `java/lang/Class.getClassLoader ()Ljava/lang/ClassLoader; -> null`

### The engine's own log lines (liblog)

- `D/JNIMain: TelemetryProtocol::setProcessTimeOverride: 0`
- `W/JNIMain: DeviceStaticParams is null.`

## M4 progress: the Android lifecycle

With `FELINE_START=1`, after `JNI_OnLoad` the runtime does what Android's
GameActivity (AGDK, Apache-2.0, open source) does, as read in AGDK's
published `GameActivity.cpp`:

1. `GameActivity.initializeNativeCode(activity, dataDir, obbDir, externalDir,
   assetManager, savedState, configuration)`: AGDK registers its 24 natives
   (`onStartNative`, `onSurfaceCreatedNative`, `onTouchEventNative`, …),
   creates the main looper, reads the configuration and calls Roblox's
   `GameActivity_onCreate`. It returns the native handle.
2. `onStartNative`, `onResumeNative`: Roblox's start-up asks for resources,
   display metrics, its `NativeHelper`, and calls `bootstrapTheApp()`.
3. The main looper runs AGDK's `mainWorkCallback` (window flags, input
   fields).
4. The engine reports `NativeHelper.gameActivity_onFlagsFailed()`: on
   Android, the Java side loads FastFlags here.

New pieces: system properties (Android 13 / SDK 33, "Feline" / "Linux"),
`ALooper` (epoll), the APK asset manager, `AConfiguration`, JNI field
access (desktop `Configuration` and `DisplayMetrics` values), object
getters returning instances of the declared class, `stat`/`lstat`/`fstat`
forwarded (bionic's x86-64 `struct stat` is the kernel's layout, as is
glibc's).

## M4 progress: a window and Roblox's app start

- **Window:** `FELINE_WINDOW=1` opens a real desktop window (winit, Wayland
  with X11 fallback) and hands the engine an `android.view.Surface` for it;
  `ANativeWindow_fromSurface/getWidth/getHeight` answer from that window, and
  AGDK's `onSurfaceCreatedNative` / `onSurfaceChangedNative` /
  `onWindowFocusChangedNative` are delivered.
- **Declarations:** `feline-runtime --natives <apk>` lists the `native` method
  declarations in the APK's DEX files (names and type signatures only; see
  CONTRIBUTING.md), which tells Feline the argument types of `Java_*` exports.
- **App start (`FELINE_APP=1`, experimental):** assets unpacked to the data
  folder; `nativeGameGlobalInit`; `nativeInitClientSettings` with an empty
  `{"applicationSettings":{}}` (accepted, returns 0); `nativeAppBridgeV2InitWithParams`;
  `nativeAppBridgeV2StartAppWithParams` with the window's surface.
- **New pieces:** the engine runs inside its data folder (it writes
  `appData/` and `cache/` relative to it); `NewObject*`, `AllocObject`,
  direct byte buffers; `Build.VERSION.SDK_INT`; `mallinfo` (= glibc's
  `mallinfo2`); semaphores on futexes (bionic's `sem_t` is 16 bytes, glibc's
  32); `setjmp`/`longjmp` forwarded (glibc's 200-byte `jmp_buf` fits
  bionic's 256).

The engine's own log then shows: controllers instantiated, the Lua app
initialized (`stage:InitializedLuaApp`), its DataModel loading
`rbxasset://places/Mobile.rbxl`. FMOD audio fails cleanly (no audio device
yet). Vulkan is declined by the engine ("Android version is too old to
activate Vulkan"), so it chooses OpenGL ES and calls `eglGetDisplay`.

### First frames

EGL now runs on the host's EGL (glvnd: NVIDIA or Mesa), see `egl.rs`:
`eglGetDisplay` becomes the window's Wayland (or X11) display,
`eglCreateWindowSurface` turns the `ANativeWindow` into a `wl_egl_window`,
Android-only config attributes are left out, and the rest of EGL and all of
GL ES are forwarded unchanged (same Khronos API on both systems).

Result (debug build, idle priority, no network): the engine reaches
`APP_READY (Landing)`, runs Roblox's Lua app UI (CoreGui, React) and
presents **650 frames in 15 s** into the Feline window. Images from the
network fail to load, as expected offline.

### Input and network

- **Input** (`keys.rs`, `window.rs`, `init.rs`): the left mouse button acts
  as one finger (`onTouchEventNative` with a `MotionEvent` whose getters
  answer from the event); keys go to `onKeyDownNative`/`onKeyUpNative` with a
  `KeyEvent` (Android `KEYCODE_*` by physical key). Call*Method now reads
  integer arguments in all three JNI forms (variadic, `va_list`, `jvalue[]`).
  `FELINE_INPUT_TEST=1` sends a synthetic tap and key press.
- **Network** (`net.rs`, `sig.rs`): `getaddrinfo`/`freeaddrinfo`/
  `gai_strerror`/`getnameinfo` translate bionic's `addrinfo` layout and BSD
  flag/error numbers; `sigaction`/`pthread_sigmask`/`sigemptyset`/
  `sigfillset` translate bionic's 8-byte `sigset_t` and its `sigaction`
  layout. The first lookups, sockets and connections are logged. With
  network access the engine resolves `*.roblox.com` and connects to
  Roblox's servers on port 443.
- The engine honours the `http_proxy`/`https_proxy`/`all_proxy` environment
  variables, like curl does: if they point to a proxy that isn't running,
  every request fails.

### Client settings, storage, and the login screen

- Without Roblox's client settings the app picks its *native* (Java) login
  screen (`APP_READY (LoginNative)`), which doesn't exist here: a white
  screen with a loading bar. Roblox's published settings for the Android
  app (`clientsettingscdn.roblox.com/v2/settings/application/AndroidApp`,
  as the app downloads them) enable the Lua login (`FFlagEnableLuaLoginRevamp8`,
  `FIntLuaAppLoginRollout = 100`). The runtime reads them from
  `<runtime>/ClientSettings.json` (or `FELINE_SETTINGS_FILE`); with them
  the app reaches `APP_READY (Landing)`, the engine-drawn landing page.
- With those settings the engine needs its cache folder:
  `NativeSettingsInterface.nativeSetCacheDirectory`, called right after
  `JNI_OnLoad`. TLS certificates: the APK's `ssl/cacert.pem`, linked to
  `files/exe/ssl` where the engine reads it.
- bionic's legacy `__sF` standard streams (`&__sF[1]` = stdout, 152-byte
  `FILE`s) are mapped to glibc's streams in `stdio.rs`.
- **Fixed: the `HttpClient` crash.** The engine has mimalloc v3 built in
  and imports no `malloc`/`free`. It calls `realpath(path, NULL)` and frees
  the result with its own `free`, which looks the pointer up in mimalloc's
  page map; glibc's heap had no entry there, so the lookup read through a
  null table (SIGSEGV at 0x1e18ca3, reading small multiples of 0x2000).
  `realpath`, `getcwd(NULL)` and `vasprintf` are now Feline's own
  (`alloc.rs`): glibc's result is copied into a buffer from the engine's
  `malloc` (offset 0x1e15298 in 2.742.0.1059, checked against its code and
  `operator new`'s call before use). Before: 6 of 6 runs crashed on Arch;
  after: 0 of 6.
- **Fixed: "Roblox no longer supports Android 6 and 7".** The engine reads
  `DeviceStaticParams.osVersion` as an API level (strtol, compared with
  `DFStringAndroidDeprecationMinUpgrade = 26` and
  `FIntGraphicsVulkanMinAndroidVersion = 25`), so it's `"33"`, not `"13"`.
  This also ends "Android version is too old to activate Vulkan" (the engine
  now looks for `libvulkan.so`, which Feline doesn't provide yet). The
  runtime also calls `NativeSettingsInterface.nativeSetDeviceInfo` with a
  `DeviceParams` (same details), as Android's Java code does at startup.
- Desktop behaviour: losing window focus isn't passed on to the engine, and
  `eglSwapInterval` is always 0 (no waits for the compositor's frame
  callbacks while the window is hidden). Neither changed the frame rate:
  about 15 per second, with the `Main` render thread never sleeping
  (about 65 ms of CPU per frame). Not yet explained.
- Diagnostics added: crash reports name the crashing thread and, outside
  the engine, the host library and symbol; the call trace records the
  calling thread and shows the crashing thread's own last calls.

**Next:** the launcher downloading the client
settings, confirming input in the window, login, audio.
