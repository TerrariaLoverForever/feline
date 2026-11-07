// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! What each of the engine's imports is bound to (M3 step 3).
//!
//! Every import gets exactly one [`Binding`]:
//!
//! - **Forward** to the host (glibc, or the system's OpenGL ES library)
//!   when bionic and glibc agree on the function's whole binary interface:
//!   the same arguments, return value and any structures passed by pointer.
//!   That holds for plain data (bytes, numbers, opaque pointers) and for
//!   structures defined by the Linux kernel (`timespec`, `sockaddr`,
//!   `epoll_event`, `dirent`, …). A few functions exist under a different
//!   glibc name ([`ALIASES`]).
//! - **Feline data** for imported *variables* Feline must own.
//! - **Stub** for everything else: functions Feline will implement in later
//!   steps because bionic and glibc differ (pthread and FILE structures,
//!   `sigset_t`, locales, `long double`, …) or because they're Android's own
//!   (assets, windows, `dlopen` of Android libraries). A stub records which
//!   function was called and stops the process, so a missing function is
//!   visible instead of silently "succeeding" (see `stubs.rs`).
//! - **Weak, empty**: optional imports left at 0, as the ELF rules allow.
//!
//! The tables below only list names whose compatibility was reasoned about
//! individually; anything not listed is a stub. Moving a name from "stub" to
//! "forward" needs that reasoning written down here.

use std::collections::BTreeMap;
use std::ffi::CString;

use crate::elf::Symbol;
use crate::host::Host;
use crate::stubs::Stubs;

/// bionic functions forwarded to glibc's function of the same name.
const FORWARD_LIBC: &[&str] = &[
    // Bytes and strings (no locale state).
    "memchr",
    "memcmp",
    "memcpy",
    "memmove",
    "memrchr",
    "memset",
    "strcasecmp",
    "strcat",
    "strchr",
    "strcmp",
    "strcpy",
    "strcspn",
    "strlen",
    "strncasecmp",
    "strncat",
    "strncmp",
    "strncpy",
    "strnlen",
    "strpbrk",
    "strrchr",
    "strspn",
    "strstr",
    "wcslen",
    "wmemchr",
    "wmemcmp",
    "bsearch",
    "qsort",
    "atof",
    "atoi",
    "atol",
    "atoll",
    "strtod",
    "strtof",
    "strtol",
    "strtoll",
    "strtoul",
    "strtoull",
    "tolower",
    "isspace",
    "btowc",
    "wctob",
    "strerror",
    // Fortified versions glibc also has, with the same arguments.
    "__memcpy_chk",
    "__memmove_chk",
    "__memset_chk",
    "__strcpy_chk",
    "__strcat_chk",
    "__strncpy_chk",
    "__vsnprintf_chk",
    "__vsprintf_chk",
    "__read_chk",
    "__readlink_chk",
    "__open_2",
    "__stack_chk_fail",
    "__cmsg_nxthdr",
    "__ctype_get_mb_cur_max",
    // Formatting into memory (va_list is the same on x86-64), and to the
    // process's own stdout.
    "snprintf",
    "vsnprintf",
    // vasprintf: Feline's own (alloc.rs), the engine frees its result.
    "sscanf",
    "vsscanf",
    "printf",
    "puts",
    // C++ runtime hooks: same arguments, keyed by the library's handle.
    "__cxa_atexit",
    "__cxa_finalize",
    "__cxa_thread_atexit_impl",
    "__register_atfork",
    // File descriptors and paths (kernel flags and numbers).
    "open",
    "close",
    "read",
    "write",
    "pread",
    "pread64",
    "pwrite",
    "writev",
    "lseek",
    "fsync",
    "ftruncate",
    "fchmod",
    "fchown",
    "fcntl",
    "ioctl",
    "unlink",
    "rmdir",
    "mkdir",
    "rename",
    "remove",
    "access",
    "readlink",
    // realpath, getcwd: Feline's own (alloc.rs), the engine frees results.
    "pipe",
    "poll",
    "select",
    "posix_fallocate",
    "utime",
    "utimes",
    // struct stat: bionic's x86-64 layout (AOSP sys/stat.h) is the kernel's,
    // field for field, as is glibc's (measured: 144 bytes, same offsets).
    "stat",
    "lstat",
    "fstat",
    // struct statvfs: eleven 8-byte fields then 24 reserved bytes (112) in
    // both (AOSP sys/statvfs.h; glibc's offsets measured).
    "statvfs",
    // Non-local jumps: bionic's jmp_buf is 256 bytes, glibc's 200 (measured),
    // so glibc's fits in the engine's buffer. The engine only hands the
    // buffer from setjmp to longjmp, never reads it, and both are forwarded,
    // so only glibc interprets it. Both libraries' `setjmp` save the signal
    // mask (BSD behaviour), and glibc's sigset_t is stored inside its 200.
    "setjmp",
    "longjmp",
    // struct hostent: name, aliases, two ints, address list, in both
    // (AOSP netdb.h); it's glibc's own static result, only read.
    // getaddrinfo & co. differ and are Feline's own (net.rs).
    "gethostbyname",
    // Directories: DIR is opaque; struct dirent is the kernel's dirent64 on x86-64.
    "opendir",
    "readdir",
    "closedir",
    // Event and timer descriptors, epoll (packed kernel struct).
    "epoll_create",
    "epoll_create1",
    "epoll_ctl",
    "epoll_wait",
    "eventfd",
    "timerfd_create",
    "timerfd_settime",
    // Memory mappings (kernel flags).
    "mmap",
    "munmap",
    "mprotect",
    "mremap",
    "msync",
    "madvise",
    "mlock",
    // Time (timespec, timeval, struct tm with tm_gmtoff/tm_zone in both).
    "clock_gettime",
    "gettimeofday",
    "time",
    "clock",
    "difftime",
    "nanosleep",
    "localtime",
    "localtime_r",
    "gmtime",
    "gmtime_r",
    "mktime",
    "strftime",
    "tzset",
    // Processes, scheduling, identity.
    "getpid",
    "getppid",
    "gettid",
    "getuid",
    "geteuid",
    "getpriority",
    "setpriority",
    "sched_yield",
    "sched_getcpu",
    "sched_get_priority_max",
    "sched_get_priority_min",
    "sched_getparam",
    "sched_getscheduler",
    "sched_setscheduler",
    "prctl",
    "syscall",
    "getpagesize",
    "getauxval",
    "uname",
    "gethostname",
    "sysinfo",
    "getenv",
    "_exit",
    "_Exit",
    "exit",
    "abort",
    "raise",
    "signal",
    "sigaltstack",
    "fork",
    "execv",
    "execve",
    "waitpid",
    "system",
    "ptrace",
    "rand",
    "srand",
    "getopt_long",
    "syslog",
    "openlog",
    "closelog",
    // Sockets (sockaddr, msghdr and option numbers are the kernel's).
    "socket",
    "socketpair",
    "bind",
    "listen",
    "accept",
    "accept4",
    "connect",
    "shutdown",
    "getsockopt",
    "setsockopt",
    "getsockname",
    "getpeername",
    "sendto",
    "recvfrom",
    "sendmsg",
    "recvmsg",
    "sendmmsg",
    "recvmmsg",
    "inet_ntop",
    "inet_pton",
    "if_indextoname",
    "if_nametoindex",
    // Randomness.
    "arc4random_buf",
    "getentropy",
];

/// POSIX threads. On x86-64, bionic's and glibc's objects have the same
/// sizes (mutex 40, cond 48, rwlock 56, attr 56 bytes; glibc's 4-byte
/// mutexattr/condattr fit bionic's 8), their default static initializers are
/// all zeros in both, and the engine only touches them through these
/// functions; with all of them forwarded, glibc alone interprets the bytes.
/// glibc's sizes were measured on the target system (gcc, `sizeof`).
///
/// Not here: `sem_*` (`sem_t` is 16 bytes in bionic, 32 in glibc; Feline's
/// own are in `sem.rs`),
/// `pthread_sigmask` (8- vs 128-byte `sigset_t`). Known gap: bionic's
/// non-default static initializers (recursive / error-checking mutexes)
/// aren't zero, and glibc wouldn't recognise them.
const FORWARD_PTHREAD: &[&str] = &[
    "pthread_attr_destroy",
    "pthread_attr_getstack",
    "pthread_attr_init",
    "pthread_attr_setdetachstate",
    "pthread_attr_setschedparam",
    "pthread_attr_setstacksize",
    "pthread_cond_broadcast",
    "pthread_cond_destroy",
    "pthread_cond_init",
    "pthread_cond_signal",
    "pthread_cond_timedwait",
    "pthread_cond_wait",
    "pthread_condattr_destroy",
    "pthread_condattr_init",
    "pthread_condattr_setclock",
    "pthread_create",
    "pthread_detach",
    "pthread_equal",
    "pthread_exit",
    "pthread_getattr_np",
    "pthread_getschedparam",
    "pthread_getspecific",
    "pthread_join",
    "pthread_key_create",
    "pthread_key_delete",
    "pthread_mutex_destroy",
    "pthread_mutex_init",
    "pthread_mutex_lock",
    "pthread_mutex_trylock",
    "pthread_mutex_unlock",
    "pthread_mutexattr_destroy",
    "pthread_mutexattr_init",
    "pthread_mutexattr_settype",
    "pthread_once",
    "pthread_rwlock_destroy",
    "pthread_rwlock_init",
    "pthread_rwlock_rdlock",
    "pthread_rwlock_unlock",
    "pthread_rwlock_wrlock",
    "pthread_self",
    "pthread_setname_np",
    "pthread_setschedparam",
    "pthread_setspecific",
];

/// stdio. `FILE` is opaque to Android apps (the NDK exposes no fields, and
/// even `getc` is imported as a function), so with every stdio function
/// forwarded, a `FILE*` is simply a glibc `FILE*`, and `stdin`/`stdout`/
/// `stderr` are glibc's own (see [`FORWARD_DATA`]). The functions taking a
/// `FILE*` are Feline's thin wrappers (stdio.rs), which also accept bionic's
/// legacy `&__sF[n]` streams; this list covers the rest and documents why
/// forwarding is right. `SEEK_*`, `_IOFBF`/
/// `_IOLBF`/`_IONBF` and `EOF` have the same values, and `off_t` is 64-bit
/// in both. `__fread_chk` has the same arguments in glibc; bionic's
/// `__fwrite_chk` doesn't exist there and is Feline's (`bionic.rs`).
const FORWARD_STDIO: &[&str] = &[
    "fopen",
    "fdopen",
    "fclose",
    "fflush",
    "fread",
    "fwrite",
    "fseek",
    "fseeko",
    "ftell",
    "ftello",
    "fgets",
    "fputs",
    "fputc",
    "getc",
    "ungetc",
    "feof",
    "ferror",
    "clearerr",
    "fileno",
    "setvbuf",
    "fprintf",
    "vfprintf",
    "fscanf",
    "getwc",
    "ungetwc",
    "fputwc",
    "__fread_chk",
];

/// Locales and multibyte text. `locale_t` is an opaque pointer in both
/// libraries, so with every function that takes one forwarded, glibc alone
/// interprets it. The `LC_*` category numbers, their `_MASK` bits and
/// `LC_GLOBAL_LOCALE` (-1) are the same in bionic and glibc. `mbstate_t` is
/// 8 bytes in both on 64-bit, zero-initialized and opaque. `struct lconv`
/// uses the POSIX field order in both. Not here: `strtold_l` (128- vs 80-bit
/// `long double`).
const FORWARD_LOCALE: &[&str] = &[
    "newlocale",
    "freelocale",
    "uselocale",
    "localeconv",
    "iswalpha_l",
    "iswblank_l",
    "iswcntrl_l",
    "iswdigit_l",
    "iswlower_l",
    "iswprint_l",
    "iswpunct_l",
    "iswspace_l",
    "iswupper_l",
    "iswxdigit_l",
    "towlower_l",
    "towupper_l",
    "strcoll_l",
    "strxfrm_l",
    "strftime_l",
    "strtoll_l",
    "strtoull_l",
    "wcscoll_l",
    "wcsxfrm_l",
    "mbrlen",
    "mbrtowc",
    "mbsnrtowcs",
    "mbsrtowcs",
    "mbtowc",
    "wcrtomb",
    "wcsnrtombs",
];

/// bionic functions forwarded to a glibc function with another name.
const ALIASES: &[(&str, &str)] = &[
    // bionic's errno accessor; glibc's returns the same thread's errno, so
    // forwarded functions and the engine see the same errno.
    ("__errno", "__errno_location"),
    // bionic's strerror_r is the POSIX one (returns int); glibc's plain
    // strerror_r is the GNU one (returns char*). bionic's GNU one has a
    // different name.
    ("strerror_r", "__xpg_strerror_r"),
    ("__gnu_strerror_r", "strerror_r"),
    // bionic's `struct mallinfo` is ten `size_t` fields; glibc's `mallinfo`
    // uses `int`, but its `mallinfo2` is the same ten `size_t` fields in the
    // same order.
    ("mallinfo", "mallinfo2"),
];

/// libm: same C math functions. `long double` versions are excluded: on
/// Android x86-64 `long double` is 128-bit, in glibc it's 80-bit.
const FORWARD_LIBM: &[&str] = &[
    "acos",
    "acosf",
    "asin",
    "asinf",
    "atan",
    "atan2",
    "atan2f",
    "atanf",
    "cbrt",
    "cbrtf",
    "ceilf",
    "cos",
    "cosf",
    "cosh",
    "coshf",
    "erfcf",
    "erff",
    "exp",
    "exp2",
    "exp2f",
    "expf",
    "expm1",
    "finitef",
    "floorf",
    "fmod",
    "fmodf",
    "frexp",
    "frexpf",
    "hypot",
    "hypotf",
    "ilogb",
    "ldexp",
    "ldexpf",
    "ldiv",
    "llround",
    "llroundf",
    "log",
    "log10",
    "log10f",
    "log2",
    "log2f",
    "logf",
    "lround",
    "lroundf",
    "modf",
    "modff",
    "nan",
    "nextafterf",
    "pow",
    "powf",
    "remainderf",
    "remquof",
    "round",
    "sin",
    "sincos",
    "sincosf",
    "sinf",
    "sinh",
    "sinhf",
    "tan",
    "tanf",
    "tanh",
    "tanhf",
];

/// Imported variables forwarded to glibc's variable of the same name and type.
const FORWARD_DATA: &[&str] = &[
    // `FILE*` variables: glibc's own, since every stdio function is glibc's.
    "stdin",
    "stdout",
    "stderr",
    "environ",
    "optarg",
    "optind",
    "tzname",
    "daylight",
    "timezone",
    "in6addr_any",
    "in6addr_loopback",
];

/// `AMEDIAFORMAT_KEY_*` (Android NDK media): `const char*` variables holding
/// the key strings, as in Android's `MediaFormat` keys.
const MEDIA_KEYS: &[(&str, &str)] = &[
    ("AMEDIAFORMAT_KEY_MIME", "mime"),
    ("AMEDIAFORMAT_KEY_WIDTH", "width"),
    ("AMEDIAFORMAT_KEY_HEIGHT", "height"),
    ("AMEDIAFORMAT_KEY_BIT_RATE", "bitrate"),
    ("AMEDIAFORMAT_KEY_SAMPLE_RATE", "sample-rate"),
    ("AMEDIAFORMAT_KEY_CHANNEL_COUNT", "channel-count"),
    ("AMEDIAFORMAT_KEY_COLOR_FORMAT", "color-format"),
    ("AMEDIAFORMAT_KEY_STRIDE", "stride"),
    ("AMEDIAFORMAT_KEY_FRAME_RATE", "frame-rate"),
    ("AMEDIAFORMAT_KEY_I_FRAME_INTERVAL", "i-frame-interval"),
];

/// Where an import ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    ForwardLibc,
    ForwardLibm,
    ForwardGles,
    ForwardData,
    FelineData,
    Feline,
    Stub,
    WeakEmpty,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::ForwardLibc => "forwarded to glibc",
            Self::ForwardLibm => "forwarded to glibc's libm",
            Self::ForwardGles => "forwarded to the system's OpenGL ES",
            Self::ForwardData => "glibc variables",
            Self::FelineData => "Feline-owned variables",
            Self::Feline => "implemented by Feline (translated from bionic to glibc)",
            Self::Stub => "stubs (Feline implements them in later steps)",
            Self::WeakEmpty => "optional, left empty",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub kind: Kind,
    pub address: u64,
}

/// Every import's binding, by name.
pub struct Imports {
    pub bindings: BTreeMap<String, Binding>,
    /// Keeps the stub code alive (and so valid) as long as the bindings.
    _stubs: Stubs,
    /// Feline-owned variables (never freed while the engine may use them).
    _data: Vec<Box<[u64]>>,
    _strings: Vec<CString>,
}

impl Imports {
    pub fn count(&self, kind: Kind) -> usize {
        self.bindings.values().filter(|b| b.kind == kind).count()
    }

    pub fn names(&self, kind: Kind) -> Vec<&str> {
        self.bindings
            .iter()
            .filter(|(_, b)| b.kind == kind)
            .map(|(n, _)| n.as_str())
            .collect()
    }
}

/// Binds every import in `symbols` (the library's dynamic symbols).
/// The function a name resolves to for the engine, by the same rules as its
/// imports (Feline's own first, then the reasoned forwarding tables, then GL
/// ES / EGL): for `dlsym` on Feline's system libraries. Variables aren't
/// resolved here.
pub fn resolve_function(name: &str, host: &Host) -> Option<u64> {
    if let Some(address) = crate::bionic::implementation(name) {
        return Some(address);
    }
    if FORWARD_LIBM.contains(&name) {
        return host.libc(name);
    }
    if FORWARD_LIBC.contains(&name)
        || FORWARD_PTHREAD.contains(&name)
        || FORWARD_LOCALE.contains(&name)
        || FORWARD_STDIO.contains(&name)
    {
        return host.libc(name);
    }
    if let Some((_, glibc)) = ALIASES.iter().find(|(b, _)| *b == name) {
        return host.libc(glibc);
    }
    if name.starts_with("gl") {
        return host.gles(name);
    }
    if name.starts_with("egl") {
        return host.egl(name);
    }
    None
}

pub fn bind(symbols: &[Symbol], host: &Host) -> Result<Imports, String> {
    let imports: Vec<&Symbol> = symbols
        .iter()
        .filter(|s| !s.defined && !s.name.is_empty())
        .collect();
    let mut bindings = BTreeMap::new();
    let mut data: Vec<Box<[u64]>> = Vec::new();
    let mut strings = Vec::new();
    let mut stub_names = Vec::new();

    // A zeroed, Feline-owned variable of `words` × 8 bytes; returns its address.
    let owned = |data: &mut Vec<Box<[u64]>>, words: usize, first: u64| {
        let mut block = vec![0u64; words].into_boxed_slice();
        block[0] = first;
        let address = block.as_ptr() as u64;
        data.push(block);
        address
    };

    for symbol in &imports {
        let name = symbol.name.as_str();
        let found =
            |kind: Kind, address: Option<u64>| address.map(|address| Binding { kind, address });
        let binding = if let Some(address) = crate::bionic::implementation(name) {
            Some(Binding {
                kind: Kind::Feline,
                address,
            })
        } else if let Some((_, value)) = MEDIA_KEYS.iter().find(|(k, _)| *k == name) {
            let text = CString::new(*value).expect("no NUL in key names");
            let pointer = text.as_ptr() as u64;
            strings.push(text);
            Some(Binding {
                kind: Kind::FelineData,
                address: owned(&mut data, 1, pointer),
            })
        } else if name == "__stack_chk_guard" {
            // A random guard value, like bionic's.
            let guard = std::hash::BuildHasher::hash_one(
                &std::collections::hash_map::RandomState::new(),
                name,
            );
            Some(Binding {
                kind: Kind::FelineData,
                address: owned(&mut data, 1, guard),
            })
        } else if name == "__sF" {
            // bionic's legacy array of three FILE objects (152 bytes each).
            // glibc has no such array: Feline owns it, and stdio.rs turns
            // pointers into it into glibc's stdin/stdout/stderr.
            let mut block = vec![0u64; 3 * crate::stdio::BIONIC_FILE_SIZE / 8].into_boxed_slice();
            crate::stdio::set_sf(&mut block);
            let address = block.as_ptr() as u64;
            data.push(block); // owned as long as the bindings, never moved
            Some(Binding {
                kind: Kind::FelineData,
                address,
            })
        } else if FORWARD_DATA.contains(&name) {
            found(Kind::ForwardData, host.libc(name))
        } else if FORWARD_LIBM.contains(&name) {
            found(Kind::ForwardLibm, host.libc(name))
        } else if FORWARD_LIBC.contains(&name)
            || FORWARD_PTHREAD.contains(&name)
            || FORWARD_LOCALE.contains(&name)
            || FORWARD_STDIO.contains(&name)
        {
            found(Kind::ForwardLibc, host.libc(name))
        } else if let Some((_, glibc)) = ALIASES.iter().find(|(b, _)| *b == name) {
            found(Kind::ForwardLibc, host.libc(glibc))
        } else if name.starts_with("gl") {
            found(Kind::ForwardGles, host.gles(name))
        } else if name.starts_with("egl") {
            // EGL is the same Khronos API on Android and Linux; the
            // platform-specific functions are Feline's own (egl.rs) and were
            // bound above.
            found(Kind::ForwardGles, host.egl(name))
        } else {
            None
        };

        let binding = match binding {
            Some(b) => b,
            // Optional imports may stay empty.
            None if symbol.bind == 2 => Binding {
                kind: Kind::WeakEmpty,
                address: 0,
            },
            // Everything else, including forwards the host lacks, is a stub.
            None => {
                stub_names.push(name.to_owned());
                continue;
            }
        };
        bindings.insert(name.to_owned(), binding);
    }

    // FELINE_CALL_TRACE=1: route every imported *function* that goes to a
    // real implementation through a recording trampoline (see trace.rs).
    // Variables are never wrapped.
    if crate::trace::enabled() {
        let functions: Vec<(String, u64)> = imports
            .iter()
            .filter(|s| s.kind == 2)
            .filter_map(|s| {
                let b = bindings.get(&s.name)?;
                matches!(
                    b.kind,
                    Kind::ForwardLibc | Kind::ForwardLibm | Kind::ForwardGles | Kind::Feline
                )
                .then(|| (s.name.clone(), b.address))
            })
            .collect();
        let tracer = Box::leak(Box::new(crate::trace::Tracer::new(functions)?));
        for (name, trampoline) in &tracer.entries {
            if let Some(binding) = bindings.get_mut(name) {
                binding.address = *trampoline;
            }
        }
    }

    let stubs = Stubs::new(stub_names)?;
    for (name, address) in stubs.addresses() {
        bindings.insert(
            name.to_owned(),
            Binding {
                kind: Kind::Stub,
                address,
            },
        );
    }
    Ok(Imports {
        bindings,
        _stubs: stubs,
        _data: data,
        _strings: strings,
    })
}

#[cfg(test)]
#[allow(unsafe_code)] // one test reads a Feline-owned variable through its address
mod tests {
    use super::*;

    #[test]
    fn tables_have_no_duplicates_or_overlaps() {
        let mut all: Vec<&str> = FORWARD_LIBC
            .iter()
            .chain(FORWARD_PTHREAD)
            .chain(FORWARD_LOCALE)
            .chain(FORWARD_STDIO)
            .chain(FORWARD_LIBM)
            .chain(FORWARD_DATA)
            .copied()
            .chain(ALIASES.iter().map(|(b, _)| *b))
            .chain(MEDIA_KEYS.iter().map(|(k, _)| *k))
            .collect();
        let before = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), before, "a name is listed twice");
    }

    #[test]
    fn known_incompatible_functions_are_never_forwarded() {
        // Each of these differs between bionic and glibc (see the module docs).
        for name in [
            "getaddrinfo",
            "sigaction",
            "sigemptyset",
            "pthread_sigmask",
            "sysconf",
            "fmal",
            "powl",
            "sem_wait",
            "sem_init",
            "sem_destroy",
            "tcsetattr",
            "__assert",
            "sigprocmask",
            "tcgetattr",
            "sem_post",
            "strtold_l",
        ] {
            assert!(
                !FORWARD_LIBC.contains(&name)
                    && !FORWARD_LIBM.contains(&name)
                    && !FORWARD_PTHREAD.contains(&name)
                    && !FORWARD_LOCALE.contains(&name)
                    && !FORWARD_STDIO.contains(&name),
                "{name}"
            );
            assert!(!ALIASES.iter().any(|(b, _)| *b == name), "{name}");
        }
        // glibc's own `mallinfo` has `int` fields; bionic's matches `mallinfo2`.
        assert!(ALIASES.contains(&("mallinfo", "mallinfo2")));
    }

    #[test]
    fn binds_every_import_of_a_small_list() {
        let symbol = |name: &str, bind: u8| Symbol {
            name: name.into(),
            value: 0,
            size: 0,
            bind,
            kind: 2,
            defined: false,
            version: None,
        };
        let symbols = [
            symbol("strlen", 1),
            symbol("__errno", 1),
            symbol("AMEDIAFORMAT_KEY_MIME", 1),
            symbol("pthread_mutex_lock", 1),
            symbol("sem_init", 1),
            symbol("tcgetattr", 1),
            symbol("ZSTD_trace_compress_begin", 2),
        ];
        let imports = bind(&symbols, &Host::open()).unwrap();
        let kind = |n: &str| imports.bindings[n].kind;
        assert_eq!(kind("strlen"), Kind::ForwardLibc);
        assert_eq!(kind("__errno"), Kind::ForwardLibc);
        assert_eq!(kind("AMEDIAFORMAT_KEY_MIME"), Kind::FelineData);
        assert_eq!(kind("pthread_mutex_lock"), Kind::ForwardLibc);
        assert_eq!(kind("sem_init"), Kind::Feline);
        assert_eq!(kind("tcgetattr"), Kind::Stub);
        assert_eq!(kind("ZSTD_trace_compress_begin"), Kind::WeakEmpty);
        assert_eq!(imports.bindings.len(), 7);
        // The media key variable holds a pointer to the string "mime".
        let variable = imports.bindings["AMEDIAFORMAT_KEY_MIME"].address as *const *const u8;
        // SAFETY (test): the binding points at a live Feline-owned u64 that
        // holds the address of a NUL-terminated CString owned by `imports`.
        let text = unsafe { std::ffi::CStr::from_ptr((*variable).cast()) };
        assert_eq!(text.to_str().unwrap(), "mime");
    }
}
