// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Feline's "Java world" (M3 step 5): the `JavaVM` and `JNIEnv` the engine
//! talks to instead of a real Java virtual machine.
//!
//! On Android, the engine lives inside a Java app and talks to it through
//! JNI: a `JNIEnv*` points to a pointer to a table of 233 C functions (the
//! order is fixed, see `jni_names.rs`), a `JavaVM*` to a table of 8. Feline
//! builds both tables:
//!
//! - Functions the engine needs to set itself up are implemented here, and
//!   **log what the engine asks for**: the Java classes it looks up, the
//!   methods and fields with their type signatures, and the native
//!   functions it registers. That log is the to-do list for M4.
//! - Classes, strings and IDs are Feline's own small numbered handles, never
//!   real memory addresses.
//! - Every other slot is a stub that names itself (e.g.
//!   `JNIEnv::CallObjectMethod`) and stops.
//!
//! # `unsafe` in this module
//!
//! These are C functions the engine calls with C pointers (strings, out
//! parameters, native method tables) that it promises are valid, as in C.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::jni_names::{JAVA_VM, JNI_ENV};
use crate::stubs::Stubs;

type JInt = i32;
type JBoolean = u8;
pub type Handle = *mut c_void;

pub const JNI_VERSION_1_6: JInt = 0x0001_0006;
const JNI_OK: JInt = 0;

/// One object in the fake Java world.
enum Object {
    Class(String),
    Str(CString),
    /// An instance of a class (e.g. the activity), with no fields.
    Instance(String),
    /// A `java.nio.DirectByteBuffer` over the engine's own memory
    /// (`NewDirectByteBuffer`): its address and size in bytes.
    Buffer {
        address: usize,
        capacity: i64,
    },
    /// A `byte[]`: Feline-owned bytes at a fixed address (never freed,
    /// like the strings), so element pointers stay valid.
    Bytes {
        address: usize,
        len: usize,
    },
    /// A `float[]` Feline made (e.g. refresh rates), never freed.
    Floats {
        address: usize,
        count: usize,
    },
}

/// A `float[]` holding `values`, for the engine.
pub fn new_float_array(values: &[f32]) -> Handle {
    let block: &'static mut [f32] = Box::leak(values.to_vec().into_boxed_slice());
    add_object(Object::Floats {
        address: block.as_mut_ptr() as usize,
        count: block.len(),
    })
}

fn floats(array: Handle) -> Option<(usize, usize)> {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(array).and_then(|i| world.objects.get(i)) {
        Some(Object::Floats { address, count }) => Some((*address, *count)),
        _ => None,
    }
}

unsafe extern "C" fn get_float_array_region(
    _env: Handle,
    array: Handle,
    start: JInt,
    len: JInt,
    buf: *mut f32,
) {
    trace("GetFloatArrayRegion");
    if let Some((address, count)) = floats(array)
        && let Some((start, len)) = range(start, len, count)
    {
        // SAFETY: inside the array; the engine's buffer holds `len` floats.
        unsafe { std::ptr::copy_nonoverlapping((address as *const f32).add(start), buf, len) };
    }
}

/// A Java String the runtime hands to the engine (e.g. a folder path).
pub fn new_string(text: &str) -> Handle {
    add_object(Object::Str(CString::new(text).unwrap_or_default()))
}

/// The class object for `class` (one per name).
pub fn new_class(class: &str) -> Handle {
    class_handle(class)
}

/// An object of class `class` (slash-separated, e.g. `android/os/Bundle`).
pub fn new_instance(class: &str) -> Handle {
    add_object(Object::Instance(class.to_owned()))
}

type Member = (&'static str, &'static str, &'static str, bool);

struct World {
    objects: Vec<Object>,
    /// The object index of each class, by name: one object per class.
    classes: BTreeMap<String, usize>,
    /// (class, name, signature, static?) per method or field ID. The texts
    /// live as long as the process (one copy per member, see `member_ids`),
    /// so looking a member up copies nothing.
    members: Vec<Member>,
    /// The ID of each member already asked for: asking again gives the
    /// same ID instead of a new entry.
    member_ids: BTreeMap<Member, usize>,
    /// (class, method name, signature) of each registered native.
    natives: Vec<(String, String, String)>,
    /// The function of each registered native, by method name.
    native_functions: Vec<(String, usize)>,
}

static WORLD: Mutex<World> = Mutex::new(World {
    objects: Vec::new(),
    classes: BTreeMap::new(),
    members: Vec::new(),
    member_ids: BTreeMap::new(),
    natives: Vec::new(),
    native_functions: Vec::new(),
});

/// Handles are `(index + 1) * 8`: never 0 (null means "none" in JNI) and
/// never a real address.
fn handle(index: usize) -> Handle {
    ((index + 1) * 8) as Handle
}

fn index(handle: Handle) -> Option<usize> {
    let h = handle as usize;
    (h != 0 && h.is_multiple_of(8)).then(|| h / 8 - 1)
}

fn add_object(object: Object) -> Handle {
    let mut world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    world.objects.push(object);
    let n = world.objects.len();
    // Always logged (it's rare): objects are never freed, so this is how a
    // long session's memory use shows up in a bug report.
    if n.is_power_of_two() && n >= 4096 {
        eprintln!("feline-runtime: jni: {n} Java objects so far");
    }
    handle(n - 1)
}

fn class_name(class: Handle) -> String {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(class).and_then(|i| world.objects.get(i)) {
        Some(Object::Class(name)) => name.clone(),
        _ => format!("<not a class: {class:p}>"),
    }
}

/// # Safety
/// `s` is NULL or a valid C string.
unsafe fn text(s: *const c_char) -> String {
    if s.is_null() {
        return "<null>".to_owned();
    }
    // SAFETY: the caller passes a valid C string.
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

/// Whether the runtime setting "Detailed logs" is on.
fn detailed() -> bool {
    static DETAILED: OnceLock<bool> = OnceLock::new();
    *DETAILED.get_or_init(|| crate::prefs::get().detailed_logs)
}

/// A line about the engine's Java calls, with the runtime setting
/// "Detailed logs" only (they were most of every log). The text is only
/// put together when it's going to be written: the engine makes these
/// calls all the time.
macro_rules! log {
    ($($arg:tt)*) => {
        if detailed() {
            eprintln!("feline-runtime: jni: {}", format_args!($($arg)*));
        }
    };
}

/// `FELINE_JNI_TRACE=1` logs every call to Feline's JNI functions (also
/// the quiet ones such as references and exception checks), numbered, up to
/// a limit, to see how the engine uses the interface.
fn trace(function: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    if !*ENABLED.get_or_init(|| std::env::var_os("FELINE_JNI_TRACE").is_some()) {
        return;
    }
    let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 200 {
        eprintln!("feline-runtime: jni trace #{n}: {function}");
    } else if n == 201 {
        eprintln!("feline-runtime: jni trace: more than 200 calls, not logging further");
    }
}

// ---------------------------------------------------------------------------
// JNIEnv functions Feline implements
// ---------------------------------------------------------------------------

extern "C" fn get_version(_env: Handle) -> JInt {
    trace("GetVersion");
    JNI_VERSION_1_6
}

unsafe extern "C" fn find_class(_env: Handle, name: *const c_char) -> Handle {
    trace("FindClass");
    // SAFETY: the engine passes a valid class name.
    let name = unsafe { text(name) };
    log!("FindClass {name}");
    class_handle(&name)
}

fn member(
    class: Handle,
    name: *const c_char,
    sig: *const c_char,
    is_static: bool,
    kind: &str,
) -> Handle {
    let class = class_name(class);
    // SAFETY: the engine passes valid C strings for name and signature.
    let (name, sig) = unsafe { (text(name), text(sig)) };
    let label = if is_static { "static " } else { "" };
    log!("Get{kind} {label}{class}.{name} {sig}");
    let mut world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(&id) = world.member_ids.get(&(class.as_str(), name.as_str(), sig.as_str(), is_static)) {
        return handle(id);
    }
    let keep = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    let member: Member = (keep(class), keep(name), keep(sig), is_static);
    let id = world.members.len();
    world.members.push(member);
    world.member_ids.insert(member, id);
    handle(id)
}

unsafe extern "C" fn get_method_id(
    _env: Handle,
    c: Handle,
    n: *const c_char,
    s: *const c_char,
) -> Handle {
    member(c, n, s, false, "MethodID")
}
unsafe extern "C" fn get_static_method_id(
    _env: Handle,
    c: Handle,
    n: *const c_char,
    s: *const c_char,
) -> Handle {
    member(c, n, s, true, "MethodID")
}
unsafe extern "C" fn get_field_id(
    _env: Handle,
    c: Handle,
    n: *const c_char,
    s: *const c_char,
) -> Handle {
    member(c, n, s, false, "FieldID")
}
unsafe extern "C" fn get_static_field_id(
    _env: Handle,
    c: Handle,
    n: *const c_char,
    s: *const c_char,
) -> Handle {
    member(c, n, s, true, "FieldID")
}

/// `JNINativeMethod`: { name, signature, function }.
#[repr(C)]
struct NativeMethod {
    name: *const c_char,
    signature: *const c_char,
    function: *const c_void,
}

unsafe extern "C" fn register_natives(
    _env: Handle,
    class: Handle,
    methods: *const NativeMethod,
    count: JInt,
) -> JInt {
    trace("RegisterNatives");
    let class = class_name(class);
    let mut world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    for i in 0..count.max(0) as usize {
        // SAFETY: the engine passes `count` valid entries.
        let m = unsafe { &*methods.add(i) };
        // SAFETY: valid C strings in each entry.
        let (name, sig) = unsafe { (text(m.name), text(m.signature)) };
        log!("  native {name} {sig}");
        world
            .native_functions
            .push((name.clone(), m.function as usize));
        world.natives.push((class.clone(), name, sig));
    }
    log!("RegisterNatives {class}: {count} natives");
    JNI_OK
}

/// The function the engine registered for `class.name` (Feline calls it as
/// Java would). `natives` and `native_functions` are filled side by side.
pub fn registered_native_of(class: &str, name: &str) -> Option<usize> {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    let at = world
        .natives
        .iter()
        .rposition(|(c, n, _)| c == class && n == name)?;
    world.native_functions.get(at).map(|(_, f)| *f)
}

/// A `java.nio.DirectByteBuffer` over `capacity` bytes at `address`, as
/// `ByteBuffer.allocateDirect` makes in Java.
pub fn new_direct_buffer(address: *mut u8, capacity: usize) -> Handle {
    add_object(Object::Buffer {
        address: address as usize,
        capacity: capacity as i64,
    })
}

/// This thread's env, attaching the thread if needed (for Feline's own
/// threads that call the engine, e.g. voice chat's audio threads).
pub fn attach_env() -> Option<Handle> {
    this_thread_env(true)
}

extern "C" fn unregister_natives(_env: Handle, _class: Handle) -> JInt {
    trace("UnregisterNatives");
    JNI_OK
}

// No Java code runs, so no Java exception is ever pending.
extern "C" fn exception_check(_env: Handle) -> JBoolean {
    trace("ExceptionCheck");
    0
}
extern "C" fn exception_occurred(_env: Handle) -> Handle {
    trace("ExceptionOccurred");
    std::ptr::null_mut()
}
extern "C" fn exception_clear(_env: Handle) {
    trace("ExceptionClear/Describe");
}

// References: Feline's objects live as long as the process, so a new
// reference is the same handle and deleting one does nothing.
extern "C" fn new_ref(_env: Handle, object: Handle) -> Handle {
    trace("New(Global|Local|WeakGlobal)Ref");
    object
}
extern "C" fn delete_ref(_env: Handle, _object: Handle) {
    trace("Delete(Global|Local|WeakGlobal)Ref");
}
extern "C" fn push_local_frame(_env: Handle, _capacity: JInt) -> JInt {
    trace("PushLocalFrame");
    JNI_OK
}
extern "C" fn pop_local_frame(_env: Handle, result: Handle) -> Handle {
    trace("PopLocalFrame");
    result
}
extern "C" fn ensure_local_capacity(_env: Handle, _capacity: JInt) -> JInt {
    trace("EnsureLocalCapacity");
    JNI_OK
}
extern "C" fn is_same_object(_env: Handle, a: Handle, b: Handle) -> JBoolean {
    trace("IsSameObject");
    JBoolean::from(a == b)
}
/// JNILocalRefType: every Feline handle behaves like a valid reference.
extern "C" fn get_object_ref_type(_env: Handle, _object: Handle) -> JInt {
    trace("GetObjectRefType");
    1
}

/// A class handle for `name` (one per name, so the same class compares
/// equal with IsSameObject).
fn class_handle(name: &str) -> Handle {
    let mut world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    class_handle_in(&mut world, name)
}

fn class_handle_in(world: &mut World, name: &str) -> Handle {
    if let Some(&i) = world.classes.get(name) {
        return handle(i);
    }
    world.objects.push(Object::Class(name.to_owned()));
    let i = world.objects.len() - 1;
    world.classes.insert(name.to_owned(), i);
    handle(i)
}

/// `GetObjectClass`: Feline's objects are classes or strings.
extern "C" fn get_object_class(_env: Handle, object: Handle) -> Handle {
    trace("GetObjectClass");
    let mut world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    let instance_class;
    let name = match index(object).and_then(|i| world.objects.get(i)) {
        Some(Object::Class(_)) => "java/lang/Class",
        Some(Object::Str(_)) => "java/lang/String",
        Some(Object::Instance(class)) => {
            instance_class = class.clone();
            instance_class.as_str()
        }
        Some(Object::Buffer { .. }) => "java/nio/DirectByteBuffer",
        Some(Object::Bytes { .. }) => "[B",
        Some(Object::Floats { .. }) => "[F",
        None => "java/lang/Object",
    };
    log!("GetObjectClass {object:p} -> {name}");
    class_handle_in(&mut world, name)
}

/// `GetSuperclass`: every Feline class extends java/lang/Object, which has
/// no superclass.
extern "C" fn get_superclass(_env: Handle, class: Handle) -> Handle {
    trace("GetSuperclass");
    let name = class_name(class);
    if name == "java/lang/Object" {
        return std::ptr::null_mut();
    }
    log!("GetSuperclass {name} -> java/lang/Object");
    class_handle("java/lang/Object")
}

/// `IsInstanceOf`: true when the object's class (see GetObjectClass) is the
/// class asked about or java/lang/Object; logged, since it's a guess about
/// a Java world that doesn't exist.
extern "C" fn is_instance_of(env: Handle, object: Handle, class: Handle) -> JBoolean {
    trace("IsInstanceOf");
    if object.is_null() {
        return 1; // JNI: NULL is an instance of every class
    }
    let actual = class_name(get_object_class(env, object));
    let wanted = class_name(class);
    let result = actual == wanted || wanted == "java/lang/Object";
    log!("IsInstanceOf {actual} of {wanted} -> {result}");
    JBoolean::from(result)
}

/// A constructor argument, read by its type in the signature.
#[derive(Debug, Clone, Copy)]
pub enum Arg {
    Int(i64),
    Float(f64),
}

/// The parameter types of a method signature, one letter each (`L…;` and
/// arrays become `L`).
fn parameter_kinds(signature: &str) -> Vec<u8> {
    let inner = signature
        .strip_prefix('(')
        .and_then(|s| s.split(')').next())
        .unwrap_or("");
    let bytes = inner.as_bytes();
    let (mut kinds, mut at) = (Vec::new(), 0);
    while at < bytes.len() {
        let mut c = bytes[at];
        while c == b'[' {
            at += 1;
            c = *bytes.get(at).unwrap_or(&b'I');
            if c != b'[' {
                if c == b'L' {
                    while at < bytes.len() && bytes[at] != b';' {
                        at += 1;
                    }
                }
                c = b'A';
            }
        }
        if c == b'L' {
            while at < bytes.len() && bytes[at] != b';' {
                at += 1;
            }
        }
        kinds.push(if c == b'A' { b'L' } else { c });
        at += 1;
    }
    kinds
}

/// A new instance of `class`. No Java constructor runs; the arguments are
/// read (by `next`, per parameter type letter) only for classes whose
/// values Feline uses: NativeTextBoxInfo, for the text box overlay.
fn construct(class: Handle, constructor: Handle, next: &mut dyn FnMut(u8) -> Arg) -> Handle {
    let name = class_name(class);
    let signature = member_of(constructor)
        .map(|(_, _, sig)| sig)
        .unwrap_or_default();
    log!("new {name} {signature} (no Java constructor runs)");
    if name == "com/roblox/engine/jni/model/NativeTextBoxInfo" {
        let args: Vec<Arg> = parameter_kinds(signature)
            .into_iter()
            .map(&mut *next)
            .collect();
        crate::text::set_info(&args);
    }
    // WebRTC's audio classes (voice chat), made with their native peer.
    if crate::voice::is_voice_class(&name) && signature == "(J)V" {
        let native = match next(b'J') {
            Arg::Int(v) => v,
            Arg::Float(_) => 0,
        };
        let object = new_instance(&name);
        crate::voice::constructed(&name, object, native);
        return object;
    }
    new_instance(&name)
}

/// `NewObject(env, class, constructor, …)`: C variadic arguments, where
/// floats arrive as doubles and smaller integers as ints.
unsafe extern "C" fn new_object(
    _env: Handle,
    class: Handle,
    constructor: Handle,
    mut args: ...
) -> Handle {
    trace("NewObject");
    construct(class, constructor, &mut |kind| match kind {
        // SAFETY: the engine passed the constructor's arguments.
        b'F' | b'D' => Arg::Float(unsafe { args.next_arg::<f64>() }),
        _ => Arg::Int(unsafe { args.next_arg::<i64>() }),
    })
}

/// `NewObjectV(env, class, constructor, va_list)`.
unsafe extern "C" fn new_object_v(
    _env: Handle,
    class: Handle,
    constructor: Handle,
    args: *mut c_void,
) -> Handle {
    trace("NewObjectV");
    construct(class, constructor, &mut |kind| match kind {
        // SAFETY: a live va_list with the constructor's arguments.
        b'F' | b'D' => Arg::Float(unsafe { va_list_double(args) }),
        _ => Arg::Int(unsafe { va_list_int(args) }),
    })
}

/// `NewObjectA(env, class, constructor, const jvalue*)`: 8 bytes per
/// argument, each in its own type.
unsafe extern "C" fn new_object_a(
    _env: Handle,
    class: Handle,
    constructor: Handle,
    args: *const c_void,
) -> Handle {
    trace("NewObjectA");
    let mut at = 0;
    construct(class, constructor, &mut |kind| {
        // SAFETY: one jvalue per argument.
        let slot = unsafe { args.cast::<u8>().add(at * 8) };
        at += 1;
        unsafe {
            match kind {
                b'F' => Arg::Float(f64::from(*slot.cast::<f32>())),
                b'D' => Arg::Float(*slot.cast::<f64>()),
                b'Z' | b'B' => Arg::Int(i64::from(*slot)),
                b'C' | b'S' => Arg::Int(i64::from(*slot.cast::<u16>())),
                b'I' => Arg::Int(i64::from(*slot.cast::<i32>())),
                _ => Arg::Int(*slot.cast::<i64>()),
            }
        }
    })
}

/// `NewDirectByteBuffer(env, address, capacity)`: a buffer object over
/// memory the engine owns (Feline only remembers where it is).
extern "C" fn new_direct_byte_buffer(_env: Handle, address: *mut c_void, capacity: i64) -> Handle {
    trace("NewDirectByteBuffer");
    add_object(Object::Buffer {
        address: address as usize,
        capacity,
    })
}

fn buffer(object: Handle) -> Option<(usize, i64)> {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(object).and_then(|i| world.objects.get(i)) {
        Some(Object::Buffer { address, capacity }) => Some((*address, *capacity)),
        _ => None,
    }
}

/// `GetDirectBufferAddress`: NULL for anything that isn't a direct buffer,
/// as JNI specifies.
extern "C" fn get_direct_buffer_address(_env: Handle, object: Handle) -> *mut c_void {
    trace("GetDirectBufferAddress");
    buffer(object).map_or(std::ptr::null_mut(), |(a, _)| a as *mut c_void)
}

/// `GetDirectBufferCapacity`: -1 for anything that isn't a direct buffer.
extern "C" fn get_direct_buffer_capacity(_env: Handle, object: Handle) -> i64 {
    trace("GetDirectBufferCapacity");
    buffer(object).map_or(-1, |(_, c)| c)
}

extern "C" fn alloc_object(_env: Handle, class: Handle) -> Handle {
    trace("AllocObject");
    let name = class_name(class);
    log!("alloc {name}");
    new_instance(&name)
}

// ---------------------------------------------------------------------------
// byte[] (the engine hands data to Java code in these, e.g. after a click).
// ---------------------------------------------------------------------------

/// The bytes of a `byte[]` handle: (address, length).
fn bytes(array: Handle) -> Option<(usize, usize)> {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(array).and_then(|i| world.objects.get(i)) {
        Some(Object::Bytes { address, len }) => Some((*address, *len)),
        _ => None,
    }
}

extern "C" fn new_byte_array(_env: Handle, len: JInt) -> Handle {
    trace("NewByteArray");
    let Ok(len) = usize::try_from(len) else {
        return std::ptr::null_mut();
    };
    let block: &'static mut [u8] = Box::leak(vec![0u8; len].into_boxed_slice());
    add_object(Object::Bytes {
        address: block.as_mut_ptr() as usize,
        len,
    })
}

/// `GetArrayLength`: Feline's byte and float arrays.
extern "C" fn get_array_length(_env: Handle, array: Handle) -> JInt {
    trace("GetArrayLength");
    bytes(array)
        .or_else(|| floats(array))
        .map_or(0, |(_, len)| len as JInt)
}

/// The range `start..start+len` of an array of `size` bytes, if inside.
fn range(start: JInt, len: JInt, size: usize) -> Option<(usize, usize)> {
    let (start, len) = (usize::try_from(start).ok()?, usize::try_from(len).ok()?);
    (start.checked_add(len)? <= size).then_some((start, len))
}

unsafe extern "C" fn get_byte_array_region(
    _env: Handle,
    array: Handle,
    start: JInt,
    len: JInt,
    buf: *mut i8,
) {
    trace("GetByteArrayRegion");
    if let Some((address, size)) = bytes(array)
        && let Some((start, len)) = range(start, len, size)
    {
        // SAFETY: inside the array; the engine's buffer holds `len` bytes.
        unsafe { std::ptr::copy_nonoverlapping((address + start) as *const i8, buf, len) };
    }
}

unsafe extern "C" fn set_byte_array_region(
    _env: Handle,
    array: Handle,
    start: JInt,
    len: JInt,
    buf: *const i8,
) {
    trace("SetByteArrayRegion");
    if let Some((address, size)) = bytes(array)
        && let Some((start, len)) = range(start, len, size)
    {
        // SAFETY: inside the array; the engine's buffer holds `len` bytes.
        unsafe { std::ptr::copy_nonoverlapping(buf, (address + start) as *mut i8, len) };
    }
}

/// `Get<Byte>ArrayElements` / `GetPrimitiveArrayCritical`: the array's own
/// bytes (not a copy), so releasing has nothing to copy back.
unsafe extern "C" fn get_array_elements(
    _env: Handle,
    array: Handle,
    is_copy: *mut JBoolean,
) -> *mut c_void {
    trace("GetByteArrayElements");
    if !is_copy.is_null() {
        // SAFETY: the engine passes a writable jboolean or NULL.
        unsafe { *is_copy = 0 };
    }
    bytes(array)
        .or_else(|| floats(array))
        .map_or(std::ptr::null_mut(), |(address, _)| address as *mut c_void)
}

extern "C" fn release_array_elements(
    _env: Handle,
    _array: Handle,
    _elements: *mut c_void,
    _mode: JInt,
) {
    trace("ReleaseByteArrayElements");
}

unsafe extern "C" fn new_string_utf(_env: Handle, s: *const c_char) -> Handle {
    trace("NewStringUTF");
    if s.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: a valid C string.
    let s = unsafe { CStr::from_ptr(s) }.to_owned();
    add_object(Object::Str(s))
}

/// Returns a pointer into Feline's copy of the string, valid for the life
/// of the process (so "release" has nothing to do).
unsafe extern "C" fn get_string_utf_chars(
    _env: Handle,
    string: Handle,
    is_copy: *mut JBoolean,
) -> *const c_char {
    trace("GetStringUTFChars");
    if !is_copy.is_null() {
        // SAFETY: the engine passes a writable jboolean or NULL.
        unsafe { *is_copy = 0 };
    }
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(string).and_then(|i| world.objects.get(i)) {
        Some(Object::Str(s)) => s.as_ptr(),
        _ => std::ptr::null(),
    }
}
extern "C" fn release_string_utf_chars(_env: Handle, _string: Handle, _chars: *const c_char) {
    trace("ReleaseStringUTFChars");
}
extern "C" fn get_string_utf_length(_env: Handle, string: Handle) -> JInt {
    trace("GetStringUTFLength");
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(string).and_then(|i| world.objects.get(i)) {
        Some(Object::Str(s)) => s.as_bytes().len() as JInt,
        _ => 0,
    }
}

unsafe extern "C" fn get_java_vm(_env: Handle, vm: *mut Handle) -> JInt {
    trace("GetJavaVM");
    // SAFETY: the engine passes a writable JavaVM* slot.
    unsafe { *vm = tables().vm };
    JNI_OK
}

// ---------------------------------------------------------------------------
// JavaVM functions
// ---------------------------------------------------------------------------

const JNI_EDETACHED: JInt = -2;

/// The thread that first asked for the JavaVM (the one that loads the
/// engine), which counts as attached from the start, like Android's main
/// thread. Other threads are attached once they call AttachCurrentThread.
static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

thread_local! {
    /// This thread's own JNIEnv: a pointer to the shared function table, at
    /// a different address on every thread, as a real VM's are. None until
    /// the thread is attached.
    static THREAD_ENV: std::cell::Cell<Option<Handle>> = const { std::cell::Cell::new(None) };
}

fn this_thread_env(attach: bool) -> Option<Handle> {
    THREAD_ENV.with(|cell| {
        if cell.get().is_none()
            && (attach || MAIN_THREAD.get() == Some(&std::thread::current().id()))
        {
            let table = tables().table;
            let env: &'static mut *const usize = Box::leak(Box::new(table));
            cell.set(Some((env as *mut *const usize).cast()));
        }
        cell.get()
    })
}

/// `GetEnv`: this thread's env, or JNI_EDETACHED if it isn't attached.
unsafe extern "C" fn vm_get_env(_vm: Handle, env: *mut Handle, _version: *mut c_void) -> JInt {
    trace("JavaVM::GetEnv");
    let current = this_thread_env(false);
    // SAFETY: the engine passes a writable JNIEnv* slot.
    unsafe { *env = current.unwrap_or(std::ptr::null_mut()) };
    if current.is_some() {
        JNI_OK
    } else {
        JNI_EDETACHED
    }
}

/// `AttachCurrentThread(AsDaemon)`: attaches (if needed) and returns the env.
unsafe extern "C" fn vm_attach(_vm: Handle, env: *mut Handle, _args: *mut c_void) -> JInt {
    trace("JavaVM::AttachCurrentThread");
    // SAFETY: the engine passes a writable JNIEnv* slot.
    unsafe { *env = this_thread_env(true).unwrap_or(std::ptr::null_mut()) };
    JNI_OK
}

extern "C" fn vm_detach(_vm: Handle) -> JInt {
    trace("JavaVM::DetachCurrentThread");
    THREAD_ENV.with(|cell| cell.set(None));
    JNI_OK
}

// ---------------------------------------------------------------------------
// Calling Java methods: Call<Type>Method, CallStatic…, CallNonvirtual…, each
// with `...`, `va_list` (V) and array (A) arguments. No Java code exists,
// so each call gets a placeholder answer from the method's signature
// (0/false, an empty String, null), logged the first few times per method.
// ---------------------------------------------------------------------------

/// The return type at the end of a JNI signature, e.g. `J` in `()J`.
fn return_type(signature: &str) -> &str {
    signature.rsplit_once(')').map_or("V", |(_, r)| r)
}

/// Logs a placeholder answer (first 3 calls per method), with "Detailed
/// logs" only.
fn answer(method: Handle, kind: &str, value: &dyn std::fmt::Display) {
    use std::collections::HashMap;
    static SEEN: Mutex<Option<HashMap<usize, u32>>> = Mutex::new(None);
    if !detailed() {
        return;
    }
    {
        let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
        let count = seen.get_or_insert_with(HashMap::new).entry(method as usize).or_insert(0);
        *count += 1;
        if *count > 3 {
            return;
        }
    }
    match member_of_full(method) {
        Some((class, name, sig, is_static)) => {
            let label = if is_static { "static " } else { "" };
            log!("{kind} {label}{class}.{name} {sig} -> {value} (placeholder; no Java code runs)");
        }
        None => log!("{kind} <unknown method {method:p}> -> {value} (placeholder; no Java code runs)"),
    }
}

/// Real answers for a few methods whose results the engine depends on
/// (everything else gets a placeholder). Set up by the runtime.
pub static KNOWN_STRINGS: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

/// A known String answer for (class, method or field), e.g. the asset path.
pub fn set_known_string(class: &str, member: &str, value: &str) {
    KNOWN_STRINGS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((class.to_owned(), member.to_owned(), value.to_owned()));
}

/// Real numeric answers for (class, method), like [`set_known_string`].
static KNOWN_NUMBERS: Mutex<Vec<(String, String, f64)>> = Mutex::new(Vec::new());

/// A known numeric answer for a method, e.g. the logged-in user's ID.
/// (Kept as f64, like every answer: exact for integers up to 2^53.)
pub fn set_known_number(class: &str, method: &str, value: f64) {
    KNOWN_NUMBERS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((class.to_owned(), method.to_owned(), value));
}

fn known_number(method: Handle) -> Option<f64> {
    let (class, name, _) = member_of(method)?;
    KNOWN_NUMBERS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(c, m, _)| *c == class && *m == name)
        .map(|(_, _, v)| *v)
}

fn known_string(class: &str, member: &str) -> Option<String> {
    KNOWN_STRINGS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(c, m, _)| c == class && m == member)
        .map(|(_, _, v)| v.clone())
}

/// (class, member) of a method or field ID.
pub(crate) fn member_of(id: Handle) -> Option<(&'static str, &'static str, &'static str)> {
    member_of_full(id).map(|(c, n, s, _)| (c, n, s))
}

/// (class, member, signature, static?) of a method or field ID.
fn member_of_full(id: Handle) -> Option<Member> {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    index(id).and_then(|i| world.members.get(i)).copied()
}

/// The placeholder for an object-returning method: an empty String for
/// `Ljava/lang/String;`, a fresh instance of the declared class for other
/// objects (on Android, getters like `getResources()` never return null),
/// and null for arrays.
fn object_answer(method: Handle, kind: &str) -> Handle {
    let (class, name, signature) = member_of(method).unwrap_or_default();
    let ret = return_type(signature);
    if let Some(value) = known_string(class, name) {
        answer(method, kind, &format_args!("\"{value}\""));
        return new_string(&value);
    }
    // Objects that are null on a normal (non-VR) device.
    if matches!(
        (class, name),
        (
            "com/roblox/engine/jni/autovalue/StartAppParams",
            "vrContext"
        )
    ) {
        answer(method, kind, &"null (not a VR device)");
        return std::ptr::null_mut();
    }
    // The window's Surface, wherever the engine asks for "the surface".
    if ret == "Landroid/view/Surface;" && crate::window::native_window().is_some() {
        answer(method, kind, &"the window's Surface");
        return crate::window::surface_object();
    }
    if ret == "Ljava/lang/String;" {
        answer(method, kind, &"\"\"");
        add_object(Object::Str(CString::default()))
    } else if let Some(class) = ret.strip_prefix('L').and_then(|r| r.strip_suffix(';')) {
        answer(method, kind, &format_args!("a new {class}"));
        new_instance(class)
    } else {
        answer(method, kind, &"null");
        std::ptr::null_mut()
    }
}

/// One family: `$v` (…), `$vv` (va_list) and `$va` (jvalue array) for an
/// instance (`obj`), static (`class`) or non-virtual (`obj, class`) call.
/// Reads the next integer argument from a C `va_list` (x86-64 System V:
/// six register slots saved in `reg_save_area`, then the stack).
///
/// # Safety
/// `list` is a live `va_list` whose next argument is an integer or pointer.
unsafe fn va_list_int(list: *mut c_void) -> i64 {
    #[repr(C)]
    struct VaListTag {
        gp_offset: u32,
        fp_offset: u32,
        overflow_arg_area: *mut u8,
        reg_save_area: *mut u8,
    }
    // SAFETY: the caller's promise; this is the ABI's va_list layout.
    unsafe {
        let tag = &mut *list.cast::<VaListTag>();
        if tag.gp_offset < 48 {
            let v = *tag.reg_save_area.add(tag.gp_offset as usize).cast::<i64>();
            tag.gp_offset += 8;
            v
        } else {
            let v = *tag.overflow_arg_area.cast::<i64>();
            tag.overflow_arg_area = tag.overflow_arg_area.add(8);
            v
        }
    }
}

/// The next floating-point argument of a `va_list` (floats are passed as
/// doubles): from the register save area while there are vector registers
/// left (offsets 48..176, 16 each), then from the stack.
///
/// # Safety
/// `list` is a live x86-64 `va_list` whose next argument is a double.
unsafe fn va_list_double(list: *mut c_void) -> f64 {
    #[repr(C)]
    struct VaListTag {
        gp_offset: u32,
        fp_offset: u32,
        overflow_arg_area: *mut u8,
        reg_save_area: *mut u8,
    }
    // SAFETY: the caller's promise; this is the ABI's va_list layout.
    unsafe {
        let tag = &mut *list.cast::<VaListTag>();
        if tag.fp_offset < 176 {
            let v = *tag.reg_save_area.add(tag.fp_offset as usize).cast::<f64>();
            tag.fp_offset += 16;
            v
        } else {
            let v = *tag.overflow_arg_area.cast::<f64>();
            tag.overflow_arg_area = tag.overflow_arg_area.add(8);
            v
        }
    }
}

/// Turning an answer (kept as f64, exact for the values used) into a JNI
/// return type.
trait FromAnswer {
    fn from_answer(value: f64) -> Self;
}
macro_rules! from_answer {
    ($($ty:ty),*) => {$(
        impl FromAnswer for $ty {
            fn from_answer(value: f64) -> Self { value as $ty }
        }
    )*};
}
from_answer!(u8, i8, u16, i16, i32, i64, f32, f64);
impl FromAnswer for () {
    fn from_answer(_value: f64) -> Self {}
}

// ---------------------------------------------------------------------------
// Input events: one reusable MotionEvent and one KeyEvent object, whose
// getters answer from the event being delivered (AGDK copies the values
// before the native call returns, so reusing them is safe).
// ---------------------------------------------------------------------------

static MOTION_EVENT: AtomicUsize = AtomicUsize::new(0);
static KEY_EVENT: AtomicUsize = AtomicUsize::new(0);
static MOTION: Mutex<(f32, f32)> = Mutex::new((0.0, 0.0));
static KEY: Mutex<Option<(crate::window::Key, i64, i64)>> = Mutex::new(None);

fn event_object(slot: &AtomicUsize, class: &str) -> Handle {
    let mut object = slot.load(Ordering::Relaxed);
    if object == 0 {
        object = new_instance(class) as usize;
        slot.store(object, Ordering::Relaxed);
    }
    object as Handle
}

/// The `android.view.MotionEvent` for one pointer at (x, y).
pub fn motion_event(x: f32, y: f32) -> Handle {
    *MOTION.lock().unwrap_or_else(|p| p.into_inner()) = (x, y);
    event_object(&MOTION_EVENT, "android/view/MotionEvent")
}

/// The `android.view.KeyEvent` for `key` (times in milliseconds of uptime,
/// as `SystemClock.uptimeMillis()`).
pub fn key_event(key: crate::window::Key, event_time: i64, down_time: i64) -> Handle {
    *KEY.lock().unwrap_or_else(|p| p.into_inner()) = Some((key, event_time, down_time));
    event_object(&KEY_EVENT, "android/view/KeyEvent")
}

/// Answers a getter of the current input event (None for other objects).
fn event_answer(object: Handle, method: Handle, read: &mut dyn FnMut() -> i64) -> Option<f64> {
    let object = object as usize;
    if object == 0 {
        return None;
    }
    if object == MOTION_EVENT.load(Ordering::Relaxed) {
        let (_, name, _) = member_of(method)?;
        let (x, y) = *MOTION.lock().unwrap_or_else(|p| p.into_inner());
        static INPUT_LOG: OnceLock<bool> = OnceLock::new();
        if *INPUT_LOG.get_or_init(|| std::env::var_os("FELINE_INPUT_LOG").is_some()) {
            log!("MotionEvent.{name} asked");
        }
        return Some(match name {
            "getPointerId" => 0.0,
            // TOOL_TYPE_MOUSE or TOOL_TYPE_FINGER (see window::mouse_mode).
            "getToolType" => {
                if crate::window::mouse_mode() {
                    3.0
                } else {
                    1.0
                }
            }
            "getRawX" | "getX" => f64::from(x),
            "getRawY" | "getY" => f64::from(y),
            "getAxisValue" => match read() as i32 {
                0 => f64::from(x), // AXIS_X
                1 => f64::from(y), // AXIS_Y
                2 => 1.0,          // AXIS_PRESSURE
                _ => 0.0,
            },
            _ => return None,
        });
    }
    if object == KEY_EVENT.load(Ordering::Relaxed) {
        let (_, name, _) = member_of(method)?;
        let (key, event_time, down_time) = (*KEY.lock().unwrap_or_else(|p| p.into_inner()))?;
        return Some(match name {
            "getDeviceId" => 1.0,
            "getSource" => f64::from(0x101), // SOURCE_KEYBOARD
            "getAction" => f64::from(u8::from(!key.down)), // ACTION_DOWN 0 / UP 1
            "getEventTime" => event_time as f64,
            "getDownTime" => down_time as f64,
            "getFlags" => 0.0,
            "getMetaState" | "getModifiers" => f64::from(key.meta),
            "getRepeatCount" => f64::from(u8::from(key.repeat)),
            "getKeyCode" => f64::from(key.code),
            "getScanCode" => f64::from(key.scan_code),
            "getUnicodeChar" => f64::from(key.unicode),
            _ => return None,
        });
    }
    None
}

/// `String.getBytes()` / `getBytes(charsetName)` on one of Feline's
/// strings: its UTF-8 bytes as a new `byte[]`. (The engine reads typed text
/// this way; Feline's strings are UTF-8, the charset it asks for.)
fn string_get_bytes(object: Handle, method: Handle) -> Option<Handle> {
    let (class, name, _) = member_of(method)?;
    if class != "java/lang/String" || name != "getBytes" {
        return None;
    }
    let text = {
        let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
        match index(object).and_then(|i| world.objects.get(i)) {
            Some(Object::Str(s)) => s.as_bytes().to_vec(),
            _ => return None,
        }
    };
    let array = new_byte_array(std::ptr::null_mut(), text.len() as JInt);
    if let Some((address, _)) = bytes(array) {
        // SAFETY: the new array holds exactly `text.len()` bytes.
        unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), address as *mut u8, text.len()) };
    }
    Some(array)
}

/// The text of one of Feline's Java strings.
pub fn string_value(object: Handle) -> Option<String> {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(object).and_then(|i| world.objects.get(i)) {
        Some(Object::Str(s)) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    }
}

/// FMOD's Java audio output, `org.fmod.AudioDevice` (see `audio.rs`): the
/// answer for `init`, and `write`/`close` passed on. None for other methods.
fn audio_call(method: Handle, read: &mut dyn FnMut() -> i64) -> Option<f64> {
    let (class, name, _) = member_of(method)?;
    if class != "org/fmod/AudioDevice" {
        return None;
    }
    match name {
        "init" => {
            let (channels, rate, size, frames) =
                (read() as i32, read() as i32, read() as i32, read() as i32);
            Some(f64::from(u8::from(crate::audio::init(channels, rate, size, frames))))
        }
        "write" => {
            let array = read() as usize as Handle;
            let length = usize::try_from(read() as i32).unwrap_or(0);
            if let Some((address, len)) = bytes(array) {
                // SAFETY: a Feline byte[] of `len` bytes, alive during the call.
                let data = unsafe { std::slice::from_raw_parts(address as *const u8, len) };
                crate::audio::write(&data[..length.min(len)]);
            }
            Some(0.0)
        }
        "close" => {
            crate::audio::close();
            Some(0.0)
        }
        _ => None,
    }
}

/// Java calls Feline acts on: the engine's on-screen keyboard requests, for
/// typing into text boxes (see `text.rs`): `showKeyboard(long textBox,
/// boolean, byte[] text, …)` and `hideKeyboard()`, on NativeGLJavaInterface
/// and NativeHelper; and the app's screen changes (`onAppReady`).
fn keyboard_call(method: Handle, read: &mut dyn FnMut() -> i64) {
    let Some((_, name, _)) = member_of(method) else {
        return;
    };
    match name {
        "showKeyboard" | "gameActivity_showKeyboard" => {
            let text_box = read();
            // showNativeInput (not "multiline", as Feline first assumed).
            let native = read() & 0xff != 0;
            let array = read() as usize as Handle;
            let initial = bytes(array)
                .map(|(address, len)| {
                    // SAFETY: a Feline byte[] of `len` bytes.
                    let raw = unsafe { std::slice::from_raw_parts(address as *const u8, len) };
                    String::from_utf8_lossy(raw).into_owned()
                })
                .unwrap_or_default();
            crate::text::focus(text_box, initial, native);
        }
        "hideKeyboard" | "gameActivity_hideKeyboard" => crate::text::blur(),
        "onLuaTextBoxPropertyChangedCallback" | "gameActivity_onLuaTextBoxPropertyChanged" => {
            crate::text::request_info_refresh();
        }
        // The engine set the focused box's text itself.
        "onLuaTextBoxChangedCallback" | "gameActivity_onLuaTextBoxChanged" => {
            if let Some(text) = string_value(read() as usize as Handle) {
                crate::text::replace_text(text);
            }
        }
        // The app's current screen ("Landing", "Login", …): the login
        // window opens on its Sign In and Create Account screens.
        "gameActivity_onAppReady" => {
            let screen = string_value(read() as usize as Handle).unwrap_or_default();
            eprintln!("feline-runtime: jni: app screen: {screen:?}");
            crate::versions::app_shown(&screen);
            crate::login::screen_shown(&screen);
        }
        _ => {}
    }
}

macro_rules! calls {
    ($kind:ident, $ty:ty, $zero:expr, $label:literal, $m:ident, $mv:ident, $ma:ident, $s:ident, $sv:ident, $sa:ident, $n:ident, $nv:ident, $na:ident) => {
        unsafe extern "C" fn $m(_env: Handle, obj: Handle, method: Handle, mut args: ...) -> $ty {
            // SAFETY: the engine passed the method's arguments.
            calls!(@body $kind, obj, method, $label, $zero, &mut || unsafe { args.next_arg::<i64>() })
        }
        unsafe extern "C" fn $mv(_env: Handle, obj: Handle, method: Handle, args: *mut c_void) -> $ty {
            // SAFETY: the engine passed a live va_list with the arguments.
            calls!(@body $kind, obj, method, $label, $zero, &mut || unsafe { va_list_int(args) })
        }
        unsafe extern "C" fn $ma(_env: Handle, obj: Handle, method: Handle, args: *const c_void) -> $ty {
            let mut at = 0;
            calls!(@body $kind, obj, method, $label, $zero, &mut || {
                // SAFETY: the engine passed one jvalue (8 bytes) per argument.
                let v = unsafe { *args.cast::<i64>().add(at) };
                at += 1;
                v
            })
        }
        unsafe extern "C" fn $s(_env: Handle, _class: Handle, method: Handle, mut args: ...) -> $ty {
            // SAFETY: the engine passed the method's arguments.
            calls!(@body $kind, std::ptr::null_mut::<c_void>(), method, $label, $zero, &mut || unsafe { args.next_arg::<i64>() })
        }
        unsafe extern "C" fn $sv(_env: Handle, _class: Handle, method: Handle, args: *mut c_void) -> $ty {
            // SAFETY: the engine passed a live va_list with the arguments.
            calls!(@body $kind, std::ptr::null_mut::<c_void>(), method, $label, $zero, &mut || unsafe { va_list_int(args) })
        }
        unsafe extern "C" fn $sa(_env: Handle, _class: Handle, method: Handle, args: *const c_void) -> $ty {
            let mut at = 0;
            calls!(@body $kind, std::ptr::null_mut::<c_void>(), method, $label, $zero, &mut || {
                // SAFETY: the engine passed one jvalue (8 bytes) per argument.
                let v = unsafe { *args.cast::<i64>().add(at) };
                at += 1;
                v
            })
        }
        unsafe extern "C" fn $n(_env: Handle, _obj: Handle, _class: Handle, method: Handle, _args: ...) -> $ty {
            calls!(@body $kind, std::ptr::null_mut::<c_void>(), method, $label, $zero, &mut || 0)
        }
        unsafe extern "C" fn $nv(_env: Handle, _obj: Handle, _class: Handle, method: Handle, _args: *mut c_void) -> $ty {
            calls!(@body $kind, std::ptr::null_mut::<c_void>(), method, $label, $zero, &mut || 0)
        }
        unsafe extern "C" fn $na(_env: Handle, _obj: Handle, _class: Handle, method: Handle, _args: *const c_void) -> $ty {
            calls!(@body $kind, std::ptr::null_mut::<c_void>(), method, $label, $zero, &mut || 0)
        }
    };
    (@body object, $obj:expr, $method:ident, $label:literal, $zero:expr, $read:expr) => {{
        trace("Call…ObjectMethod");
        let _ = $read;
        if let Some(bytes) = string_get_bytes($obj, $method) {
            return bytes;
        }
        object_answer($method, "call")
    }};
    (@body value, $obj:expr, $method:ident, $label:literal, $zero:expr, $read:expr) => {{
        trace(concat!("Call…", $label, "Method"));
        if let Some((class, name, signature)) = member_of($method) {
            // Roblox's message bus calling one of Feline's handlers.
            if crate::messagebus::call($obj, &name, &signature, $read) {
                #[allow(clippy::unused_unit)]
                return $zero;
            }
            // WebRTC's audio classes (voice chat).
            if let Some(value) = crate::voice::call(&class, $obj, &name, $read) {
                return FromAnswer::from_answer(value);
            }
        }
        keyboard_call($method, $read);
        if let Some(value) = event_answer($obj, $method, $read) {
            return FromAnswer::from_answer(value);
        }
        if let Some(value) = audio_call($method, $read) {
            return FromAnswer::from_answer(value);
        }
        if let Some(value) = known_number($method) {
            answer($method, "call", &value);
            return FromAnswer::from_answer(value);
        }
        answer($method, "call", &format_args!("{:?}", $zero));
        $zero
    }};
}

calls!(
    object,
    Handle,
    std::ptr::null_mut::<c_void>(),
    "Object",
    call_obj,
    call_obj_v,
    call_obj_a,
    call_s_obj,
    call_s_obj_v,
    call_s_obj_a,
    call_n_obj,
    call_n_obj_v,
    call_n_obj_a
);
calls!(
    value,
    JBoolean,
    0u8,
    "Boolean",
    call_bool,
    call_bool_v,
    call_bool_a,
    call_s_bool,
    call_s_bool_v,
    call_s_bool_a,
    call_n_bool,
    call_n_bool_v,
    call_n_bool_a
);
calls!(
    value,
    i8,
    0i8,
    "Byte",
    call_byte,
    call_byte_v,
    call_byte_a,
    call_s_byte,
    call_s_byte_v,
    call_s_byte_a,
    call_n_byte,
    call_n_byte_v,
    call_n_byte_a
);
calls!(
    value,
    u16,
    0u16,
    "Char",
    call_char,
    call_char_v,
    call_char_a,
    call_s_char,
    call_s_char_v,
    call_s_char_a,
    call_n_char,
    call_n_char_v,
    call_n_char_a
);
calls!(
    value,
    i16,
    0i16,
    "Short",
    call_short,
    call_short_v,
    call_short_a,
    call_s_short,
    call_s_short_v,
    call_s_short_a,
    call_n_short,
    call_n_short_v,
    call_n_short_a
);
calls!(
    value,
    JInt,
    0i32,
    "Int",
    call_int,
    call_int_v,
    call_int_a,
    call_s_int,
    call_s_int_v,
    call_s_int_a,
    call_n_int,
    call_n_int_v,
    call_n_int_a
);
calls!(
    value,
    i64,
    0i64,
    "Long",
    call_long,
    call_long_v,
    call_long_a,
    call_s_long,
    call_s_long_v,
    call_s_long_a,
    call_n_long,
    call_n_long_v,
    call_n_long_a
);
calls!(
    value,
    f32,
    0f32,
    "Float",
    call_float,
    call_float_v,
    call_float_a,
    call_s_float,
    call_s_float_v,
    call_s_float_a,
    call_n_float,
    call_n_float_v,
    call_n_float_a
);
calls!(
    value,
    f64,
    0f64,
    "Double",
    call_double,
    call_double_v,
    call_double_a,
    call_s_double,
    call_s_double_v,
    call_s_double_a,
    call_n_double,
    call_n_double_v,
    call_n_double_a
);
calls!(
    value,
    (),
    (),
    "Void",
    call_void,
    call_void_v,
    call_void_a,
    call_s_void,
    call_s_void_v,
    call_s_void_a,
    call_n_void,
    call_n_void_v,
    call_n_void_a
);

/// The address of a Call* function by its JNI name, e.g. `CallStaticLongMethodV`.
fn call_function(name: &str) -> Option<usize> {
    let rest = name
        .strip_prefix("Call")?
        .strip_suffix("Method")
        .map(|r| (r, ""))
        .or_else(|| {
            name.strip_prefix("Call")?
                .strip_suffix("MethodV")
                .map(|r| (r, "V"))
        })
        .or_else(|| {
            name.strip_prefix("Call")?
                .strip_suffix("MethodA")
                .map(|r| (r, "A"))
        })?;
    let (middle, style) = rest;
    let (scope, ty) = if let Some(t) = middle.strip_prefix("Static") {
        ("s", t)
    } else if let Some(t) = middle.strip_prefix("Nonvirtual") {
        ("n", t)
    } else {
        ("", middle)
    };
    macro_rules! pick {
        ($i:ident, $iv:ident, $ia:ident, $s:ident, $sv:ident, $sa:ident, $n:ident, $nv:ident, $na:ident) => {
            Some(match (scope, style) {
                ("", "") => $i as *const () as usize,
                ("", "V") => $iv as *const () as usize,
                ("", "A") => $ia as *const () as usize,
                ("s", "") => $s as *const () as usize,
                ("s", "V") => $sv as *const () as usize,
                ("s", "A") => $sa as *const () as usize,
                ("n", "") => $n as *const () as usize,
                ("n", "V") => $nv as *const () as usize,
                ("n", "A") => $na as *const () as usize,
                _ => return None,
            })
        };
    }
    match ty {
        "Object" => pick!(
            call_obj,
            call_obj_v,
            call_obj_a,
            call_s_obj,
            call_s_obj_v,
            call_s_obj_a,
            call_n_obj,
            call_n_obj_v,
            call_n_obj_a
        ),
        "Boolean" => pick!(
            call_bool,
            call_bool_v,
            call_bool_a,
            call_s_bool,
            call_s_bool_v,
            call_s_bool_a,
            call_n_bool,
            call_n_bool_v,
            call_n_bool_a
        ),
        "Byte" => pick!(
            call_byte,
            call_byte_v,
            call_byte_a,
            call_s_byte,
            call_s_byte_v,
            call_s_byte_a,
            call_n_byte,
            call_n_byte_v,
            call_n_byte_a
        ),
        "Char" => pick!(
            call_char,
            call_char_v,
            call_char_a,
            call_s_char,
            call_s_char_v,
            call_s_char_a,
            call_n_char,
            call_n_char_v,
            call_n_char_a
        ),
        "Short" => pick!(
            call_short,
            call_short_v,
            call_short_a,
            call_s_short,
            call_s_short_v,
            call_s_short_a,
            call_n_short,
            call_n_short_v,
            call_n_short_a
        ),
        "Int" => pick!(
            call_int,
            call_int_v,
            call_int_a,
            call_s_int,
            call_s_int_v,
            call_s_int_a,
            call_n_int,
            call_n_int_v,
            call_n_int_a
        ),
        "Long" => pick!(
            call_long,
            call_long_v,
            call_long_a,
            call_s_long,
            call_s_long_v,
            call_s_long_a,
            call_n_long,
            call_n_long_v,
            call_n_long_a
        ),
        "Float" => pick!(
            call_float,
            call_float_v,
            call_float_a,
            call_s_float,
            call_s_float_v,
            call_s_float_a,
            call_n_float,
            call_n_float_v,
            call_n_float_a
        ),
        "Double" => pick!(
            call_double,
            call_double_v,
            call_double_a,
            call_s_double,
            call_s_double_v,
            call_s_double_a,
            call_n_double,
            call_n_double_v,
            call_n_double_a
        ),
        "Void" => pick!(
            call_void,
            call_void_v,
            call_void_a,
            call_s_void,
            call_s_void_v,
            call_s_void_a,
            call_n_void,
            call_n_void_v,
            call_n_void_a
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Fields: Get<Type>Field / GetStatic<Type>Field (and Set…, which are
// accepted and logged). No Java objects hold data, so values come from a
// small table describing a desktop "device" (for android.content.res.
// Configuration, which AGDK reads), otherwise 0 / null, logged.
// ---------------------------------------------------------------------------

/// Known field values: (class, field) → value.
fn known_field(class: &str, field: &str) -> Option<f64> {
    Some(match (class, field) {
        // The window's size in dp (pixels at 160 dpi), as Android reports
        // for an app in a window.
        ("android/content/res/Configuration", "screenWidthDp") => f64::from(window_px().0),
        ("android/content/res/Configuration", "screenHeightDp") => f64::from(window_px().1),
        ("android/content/res/Configuration", "smallestScreenWidthDp") => {
            f64::from(window_px().0.min(window_px().1))
        }
        ("android/content/res/Configuration", "densityDpi") => 160.0,
        ("android/content/res/Configuration", "fontScale") => 1.0,
        // ORIENTATION_LANDSCAPE
        ("android/content/res/Configuration", "orientation") => 2.0,
        // TOUCHSCREEN_NOTOUCH, KEYBOARD_QWERTY, NAVIGATION_NONAV: a desktop
        // with keyboard and mouse. (FINGER, 3, was needed while the mouse
        // arrived as a finger; it now goes through Roblox's mouse input.
        // With FINGER, games that guess the device from its inputs saw
        // touch, keyboard and mouse at once and called the OS "unknown".)
        ("android/content/res/Configuration", "touchscreen") => 1.0,
        ("android/content/res/Configuration", "keyboard") => 2.0,
        ("android/content/res/Configuration", "navigation") => 1.0,
        // KEYBOARDHIDDEN_NO, HARDKEYBOARDHIDDEN_NO: a physical keyboard is
        // attached and usable (as with a keyboard on an Android tablet).
        ("android/content/res/Configuration", "keyboardHidden") => 1.0,
        ("android/content/res/Configuration", "hardKeyboardHidden") => 1.0,
        // UI_MODE_TYPE_NORMAL | UI_MODE_NIGHT_NO
        ("android/content/res/Configuration", "uiMode") => 0x11 as f64,
        // A 1920x1080 desktop screen at Android's baseline density.
        ("android/util/DisplayMetrics", "density" | "scaledDensity") => 1.0,
        ("android/util/DisplayMetrics", "densityDpi" | "xdpi" | "ydpi") => 160.0,
        ("android/util/DisplayMetrics", "widthPixels") => 1920.0,
        ("android/util/DisplayMetrics", "heightPixels") => 1080.0,
        // Android 13, matching ro.build.version.sdk.
        ("android/os/Build$VERSION", "SDK_INT") => 33.0,
        ("com/roblox/engine/jni/model/DeviceStaticParams", "cpu64Bit") => 1.0,
        // nativeSetDeviceInfo's DeviceParams: the same 1920x1080 screen, and
        // a high-end phone's memory classes (MB, ActivityManager's). The
        // low-memory-killer thresholds stay 0 (unknown).
        ("com/roblox/engine/jni/model/DeviceParams", "cpu64Bit") => 1.0,
        ("com/roblox/engine/jni/model/DeviceParams", "displayPhysicalWidthPixels") => 1920.0,
        ("com/roblox/engine/jni/model/DeviceParams", "displayPhysicalHeightPixels") => 1080.0,
        ("com/roblox/engine/jni/model/DeviceParams", "memoryClass") => 512.0,
        ("com/roblox/engine/jni/model/DeviceParams", "largeMemoryClass") => 1024.0,
        ("com/roblox/engine/jni/model/PlatformParams", "dpiScale") => 1.0,
        // The window's physical size at 160 dpi: pixels / 160 * 25.4 mm.
        ("com/roblox/engine/jni/model/PlatformParams", "viewportWidthMm") => window_mm(true),
        ("com/roblox/engine/jni/model/PlatformParams", "viewportHeightMm") => window_mm(false),
        _ => return None,
    })
}

/// The window's width or height in millimetres at 160 dpi (1920x1080 if
/// there's no window).
/// The window's size in pixels (1920x1080 before it exists).
fn window_px() -> (i32, i32) {
    crate::window::native_window().map_or((1920, 1080), |w| w.size())
}

fn window_mm(width: bool) -> f64 {
    let (w, h) = window_px();
    f64::from(if width { w } else { h }) / 160.0 * 25.4
}

/// The value for a field ID, logged the first few times per field.
fn field_value(field: Handle, kind: &str) -> f64 {
    use std::collections::HashMap;
    static SEEN: Mutex<Option<HashMap<usize, u32>>> = Mutex::new(None);
    let member = member_of_full(field);
    let value = member.and_then(|(class, name, _, _)| known_field(class, name));
    if detailed() {
        let first_few = {
            let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
            let count = seen.get_or_insert_with(HashMap::new).entry(field as usize).or_insert(0);
            *count += 1;
            *count <= 3
        };
        if first_few {
            let description = match member {
                Some((class, name, sig, _)) => format!("{class}.{name} {sig}"),
                None => format!("<unknown field {field:p}>"),
            };
            match value {
                Some(v) => log!("{kind} {description} -> {v}"),
                None => log!("{kind} {description} -> 0 (placeholder; no Java data)"),
            }
        }
    }
    value.unwrap_or(0.0)
}

macro_rules! field_getters {
    ($($get:ident, $get_static:ident, $ty:ty, $convert:expr;)*) => {$(
        extern "C" fn $get(_env: Handle, _object: Handle, field: Handle) -> $ty {
            trace(stringify!($get));
            let convert: fn(f64) -> $ty = $convert;
            convert(field_value(field, "get"))
        }
        extern "C" fn $get_static(_env: Handle, _class: Handle, field: Handle) -> $ty {
            trace(stringify!($get_static));
            let convert: fn(f64) -> $ty = $convert;
            convert(field_value(field, "get static"))
        }
    )*};
}

field_getters! {
    get_bool_field, get_static_bool_field, JBoolean, |v| JBoolean::from(v != 0.0);
    get_byte_field, get_static_byte_field, i8, |v| v as i8;
    get_char_field, get_static_char_field, u16, |v| v as u16;
    get_short_field, get_static_short_field, i16, |v| v as i16;
    get_int_field, get_static_int_field, JInt, |v| v as JInt;
    get_long_field, get_static_long_field, i64, |v| v as i64;
    get_float_field, get_static_float_field, f32, |v| v as f32;
    get_double_field, get_static_double_field, f64, |v| v;
}

/// Object fields follow the same rule as object-returning methods: "" for
/// Strings, a fresh instance of the declared class for objects, null for
/// arrays.
extern "C" fn get_object_field(_env: Handle, _object: Handle, field: Handle) -> Handle {
    trace("GetObjectField");
    field_value(field, "get");
    let (class, name, ty) = member_of(field).unwrap_or_default();
    if let Some(value) = known_string(class, name) {
        log!("get {class}.{name} -> \"{value}\"");
        return new_string(&value);
    }
    if ty == "Ljava/lang/String;" {
        add_object(Object::Str(CString::default()))
    } else if let Some(class) = ty.strip_prefix('L').and_then(|t| t.strip_suffix(';')) {
        new_instance(class)
    } else {
        std::ptr::null_mut()
    }
}

/// Set<Type>Field / SetStatic<Type>Field: nothing stores Java data, so the
/// value is dropped (the call is traced).
extern "C" fn set_field(_env: Handle, _object: Handle, _field: Handle, _value: u64) {
    trace("Set…Field");
}

fn field_function(name: &str) -> Option<usize> {
    type Getter<T> = extern "C" fn(Handle, Handle, Handle) -> T;
    Some(match name {
        "GetBooleanField" => get_bool_field as Getter<JBoolean> as usize,
        "GetByteField" => get_byte_field as Getter<i8> as usize,
        "GetCharField" => get_char_field as Getter<u16> as usize,
        "GetShortField" => get_short_field as Getter<i16> as usize,
        "GetIntField" => get_int_field as Getter<JInt> as usize,
        "GetLongField" => get_long_field as Getter<i64> as usize,
        "GetFloatField" => get_float_field as Getter<f32> as usize,
        "GetDoubleField" => get_double_field as Getter<f64> as usize,
        "GetObjectField" | "GetStaticObjectField" => get_object_field as Getter<Handle> as usize,
        "GetStaticBooleanField" => get_static_bool_field as Getter<JBoolean> as usize,
        "GetStaticByteField" => get_static_byte_field as Getter<i8> as usize,
        "GetStaticCharField" => get_static_char_field as Getter<u16> as usize,
        "GetStaticShortField" => get_static_short_field as Getter<i16> as usize,
        "GetStaticIntField" => get_static_int_field as Getter<JInt> as usize,
        "GetStaticLongField" => get_static_long_field as Getter<i64> as usize,
        "GetStaticFloatField" => get_static_float_field as Getter<f32> as usize,
        "GetStaticDoubleField" => get_static_double_field as Getter<f64> as usize,
        // Float/double setters pass the value in a vector register, which a
        // function that ignores it doesn't touch either.
        n if n.starts_with("Set") && n.ends_with("Field") => {
            set_field as extern "C" fn(Handle, Handle, Handle, u64) as usize
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// Feline's implementation for a slot, by its JNI name.
fn implemented(name: &str) -> Option<usize> {
    if let Some(address) = call_function(name) {
        return Some(address);
    }
    if let Some(address) = field_function(name) {
        return Some(address);
    }
    type MemberId = unsafe extern "C" fn(Handle, Handle, *const c_char, *const c_char) -> Handle;
    type RefFn = extern "C" fn(Handle, Handle) -> Handle;
    type DelFn = extern "C" fn(Handle, Handle);
    Some(match name {
        "GetVersion" => get_version as extern "C" fn(Handle) -> JInt as usize,
        "FindClass" => find_class as unsafe extern "C" fn(Handle, *const c_char) -> Handle as usize,
        "GetMethodID" => get_method_id as MemberId as usize,
        "GetStaticMethodID" => get_static_method_id as MemberId as usize,
        "GetFieldID" => get_field_id as MemberId as usize,
        "GetStaticFieldID" => get_static_field_id as MemberId as usize,
        "RegisterNatives" => {
            register_natives
                as unsafe extern "C" fn(Handle, Handle, *const NativeMethod, JInt) -> JInt
                as usize
        }
        "UnregisterNatives" => unregister_natives as extern "C" fn(Handle, Handle) -> JInt as usize,
        "ExceptionCheck" => exception_check as extern "C" fn(Handle) -> JBoolean as usize,
        "ExceptionOccurred" => exception_occurred as extern "C" fn(Handle) -> Handle as usize,
        "ExceptionClear" | "ExceptionDescribe" => exception_clear as extern "C" fn(Handle) as usize,
        "NewGlobalRef" | "NewLocalRef" | "NewWeakGlobalRef" => new_ref as RefFn as usize,
        "DeleteGlobalRef" | "DeleteLocalRef" | "DeleteWeakGlobalRef" => {
            delete_ref as DelFn as usize
        }
        "PushLocalFrame" => push_local_frame as extern "C" fn(Handle, JInt) -> JInt as usize,
        "PopLocalFrame" => pop_local_frame as RefFn as usize,
        "EnsureLocalCapacity" => {
            ensure_local_capacity as extern "C" fn(Handle, JInt) -> JInt as usize
        }
        "IsSameObject" => {
            is_same_object as extern "C" fn(Handle, Handle, Handle) -> JBoolean as usize
        }
        "GetObjectRefType" => get_object_ref_type as extern "C" fn(Handle, Handle) -> JInt as usize,
        "NewByteArray" => new_byte_array as extern "C" fn(Handle, JInt) -> Handle as usize,
        "GetArrayLength" => get_array_length as extern "C" fn(Handle, Handle) -> JInt as usize,
        "GetByteArrayRegion" => {
            get_byte_array_region as unsafe extern "C" fn(Handle, Handle, JInt, JInt, *mut i8)
                as usize
        }
        "SetByteArrayRegion" => {
            set_byte_array_region as unsafe extern "C" fn(Handle, Handle, JInt, JInt, *const i8)
                as usize
        }
        "GetFloatArrayRegion" => {
            get_float_array_region as unsafe extern "C" fn(Handle, Handle, JInt, JInt, *mut f32)
                as usize
        }
        "GetByteArrayElements" | "GetFloatArrayElements" | "GetPrimitiveArrayCritical" => {
            get_array_elements as unsafe extern "C" fn(Handle, Handle, *mut JBoolean) -> *mut c_void
                as usize
        }
        "ReleaseByteArrayElements" | "ReleaseFloatArrayElements" | "ReleasePrimitiveArrayCritical" => {
            release_array_elements as extern "C" fn(Handle, Handle, *mut c_void, JInt) as usize
        }
        "NewStringUTF" => {
            new_string_utf as unsafe extern "C" fn(Handle, *const c_char) -> Handle as usize
        }
        "GetStringUTFChars" => {
            get_string_utf_chars
                as unsafe extern "C" fn(Handle, Handle, *mut JBoolean) -> *const c_char
                as usize
        }
        "ReleaseStringUTFChars" => {
            release_string_utf_chars as extern "C" fn(Handle, Handle, *const c_char) as usize
        }
        "GetStringUTFLength" => {
            get_string_utf_length as extern "C" fn(Handle, Handle) -> JInt as usize
        }
        "GetObjectClass" => get_object_class as extern "C" fn(Handle, Handle) -> Handle as usize,
        "GetSuperclass" => get_superclass as extern "C" fn(Handle, Handle) -> Handle as usize,
        "IsInstanceOf" => {
            is_instance_of as extern "C" fn(Handle, Handle, Handle) -> JBoolean as usize
        }
        "NewDirectByteBuffer" => {
            new_direct_byte_buffer as extern "C" fn(Handle, *mut c_void, i64) -> Handle as usize
        }
        "GetDirectBufferAddress" => {
            get_direct_buffer_address as extern "C" fn(Handle, Handle) -> *mut c_void as usize
        }
        "GetDirectBufferCapacity" => {
            get_direct_buffer_capacity as extern "C" fn(Handle, Handle) -> i64 as usize
        }
        "AllocObject" => alloc_object as extern "C" fn(Handle, Handle) -> Handle as usize,
        "NewObject" => {
            new_object as unsafe extern "C" fn(Handle, Handle, Handle, ...) -> Handle as usize
        }
        "NewObjectV" => {
            new_object_v as unsafe extern "C" fn(Handle, Handle, Handle, *mut c_void) -> Handle
                as usize
        }
        "NewObjectA" => {
            new_object_a as unsafe extern "C" fn(Handle, Handle, Handle, *const c_void) -> Handle
                as usize
        }
        "GetJavaVM" => get_java_vm as unsafe extern "C" fn(Handle, *mut Handle) -> JInt as usize,
        _ => return None,
    })
}

struct Tables {
    /// The `JavaVM*` handed to the engine: points to a pointer to its table.
    vm: Handle,
    /// The JNIEnv function table itself (shared by every thread's env).
    table: *const usize,
}

// The pointers point at leaked, never-freed memory, read-only after setup.
unsafe impl Send for Tables {}
unsafe impl Sync for Tables {}

static TABLES: OnceLock<Tables> = OnceLock::new();

fn build() -> Result<Tables, String> {
    let env_missing: Vec<String> = JNI_ENV
        .iter()
        .filter(|n| !n.starts_with("reserved") && implemented(n).is_none())
        .map(|n| format!("JNIEnv::{n}"))
        .collect();
    let vm_missing: Vec<String> = JAVA_VM
        .iter()
        .filter(|n| !n.starts_with("reserved") && vm_implemented(n).is_none())
        .map(|n| format!("JavaVM::{n}"))
        .collect();
    let stubs = Box::leak(Box::new(Stubs::new(
        env_missing.into_iter().chain(vm_missing).collect(),
    )?));
    let stub = |name: String| {
        stubs
            .addresses()
            .find(|(n, _)| *n == name.as_str())
            .map(|(_, a)| a as usize)
    };

    let mut env_table = Box::new([0usize; JNI_ENV.len()]);
    for (slot, name) in JNI_ENV.iter().enumerate() {
        if !name.starts_with("reserved") {
            env_table[slot] = implemented(name)
                .or_else(|| stub(format!("JNIEnv::{name}")))
                .ok_or("missing JNIEnv stub")?;
        }
    }
    let mut vm_table = Box::new([0usize; JAVA_VM.len()]);
    for (slot, name) in JAVA_VM.iter().enumerate() {
        if !name.starts_with("reserved") {
            vm_table[slot] = vm_implemented(name)
                .or_else(|| stub(format!("JavaVM::{name}")))
                .ok_or("missing JavaVM stub")?;
        }
    }
    // JNIEnv* → (pointer to table) → table; likewise JavaVM*.
    let table: *const usize = Box::leak(env_table).as_ptr();
    let vm_ptr: &'static mut *const usize = Box::leak(Box::new(Box::leak(vm_table).as_ptr()));
    Ok(Tables {
        vm: (vm_ptr as *mut *const usize).cast(),
        table,
    })
}

fn vm_implemented(name: &str) -> Option<usize> {
    type GetEnv = unsafe extern "C" fn(Handle, *mut Handle, *mut c_void) -> JInt;
    Some(match name {
        "GetEnv" => vm_get_env as GetEnv as usize,
        "AttachCurrentThread" | "AttachCurrentThreadAsDaemon" => vm_attach as GetEnv as usize,
        "DetachCurrentThread" => vm_detach as extern "C" fn(Handle) -> JInt as usize,
        _ => return None,
    })
}

fn tables() -> &'static Tables {
    TABLES
        .get()
        .expect("JNI tables are built before the engine can call them")
}

/// The `JavaVM*` for the engine (builds the tables on first use).
pub fn java_vm() -> Result<Handle, String> {
    let _ = MAIN_THREAD.set(std::thread::current().id());
    if TABLES.get().is_none() {
        let built = build()?;
        let _ = TABLES.set(built);
    }
    Ok(tables().vm)
}

/// The function the engine registered for the native method `name` (e.g.
/// `onResumeNative`), if it did.
pub fn registered_native(name: &str) -> Option<usize> {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    world
        .native_functions
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, f)| *f)
}

/// This thread's JNIEnv (the loading thread counts as attached).
pub fn current_env() -> Option<Handle> {
    this_thread_env(false)
}

/// What the engine asked of the Java world so far.
pub struct Summary {
    pub classes: Vec<String>,
    pub members: usize,
    pub natives: Vec<(String, String, String)>,
}

pub fn summary() -> Summary {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    let mut classes: Vec<String> = world
        .objects
        .iter()
        .filter_map(|o| match o {
            Object::Class(name) => Some(name.clone()),
            Object::Str(_)
            | Object::Instance(_)
            | Object::Buffer { .. }
            | Object::Bytes { .. }
            | Object::Floats { .. } => None,
        })
        .collect();
    classes.sort();
    classes.dedup();
    Summary {
        classes,
        members: world.members.len(),
        natives: world.natives.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `count` integers from its own variadic arguments through a C
    /// `va_list`, the way an engine's `Call…MethodV` hands them over.
    unsafe extern "C" fn sum_through_va_list(count: i64, args: ...) -> i64 {
        unsafe extern "C" fn reader(count: i64, list: *mut c_void) -> i64 {
            // SAFETY (test): `list` holds `count` integer arguments.
            (0..count)
                .map(|_| unsafe { va_list_int(list) })
                .fold(0, |a, v| a * 10 + v)
        }
        // A VaList is passed exactly as C passes a va_list (a pointer).
        // SAFETY (test): same ABI, as with vsnprintf in liblog.rs.
        let reader: unsafe extern "C" fn(i64, std::ffi::VaList) -> i64 =
            unsafe { std::mem::transmute(reader as unsafe extern "C" fn(i64, *mut c_void) -> i64) };
        // SAFETY (test): as above.
        unsafe { reader(count, args) }
    }

    #[test]
    fn va_list_integers_from_registers_and_stack() {
        // 1 fixed + 8 variadic: five in registers, three on the stack.
        // SAFETY (test): eight integer arguments, as declared.
        let digits =
            unsafe { sum_through_va_list(8, 1i64, 2i64, 3i64, 4i64, 5i64, 6i64, 7i64, 8i64) };
        assert_eq!(digits, 12_345_678);
    }

    #[test]
    fn tables_have_every_slot_in_the_right_place() {
        let vm = java_vm().unwrap();
        // SAFETY (test): `vm` points to a pointer to the 8-slot table.
        let vm_table = unsafe { std::slice::from_raw_parts(*(vm as *const *const usize), 8) };
        assert_eq!(&vm_table[..3], &[0, 0, 0], "reserved slots are NULL");
        assert!(vm_table[3..].iter().all(|&f| f != 0));

        // GetEnv (slot 6) hands out the JNIEnv; FindClass (slot 6) works.
        let get_env: unsafe extern "C" fn(Handle, *mut Handle, *mut c_void) -> JInt =
            // SAFETY (test): slot 6 is GetEnv, by the generated table.
            unsafe { std::mem::transmute(vm_table[6]) };
        let mut env: Handle = std::ptr::null_mut();
        // SAFETY (test): valid out pointer.
        assert_eq!(
            unsafe { get_env(vm, &mut env, std::ptr::null_mut()) },
            JNI_OK
        );
        // SAFETY (test): `env` points to a pointer to the 233-slot table.
        let env_table = unsafe { std::slice::from_raw_parts(*(env as *const *const usize), 233) };
        let find_class: unsafe extern "C" fn(Handle, *const c_char) -> Handle =
            // SAFETY (test): slot 6 is FindClass.
            unsafe { std::mem::transmute(env_table[6]) };
        // SAFETY (test): a valid class name.
        let class = unsafe { find_class(env, c"com/roblox/Test".as_ptr()) };
        assert_eq!(class_name(class), "com/roblox/Test");
        assert_eq!(JNI_ENV[6], "FindClass");
        assert_eq!(JNI_ENV[215], "RegisterNatives");
        assert_eq!(JAVA_VM[6], "GetEnv");

        // Every one of the 90 Call* slots is implemented.
        let calls: Vec<&&str> = JNI_ENV.iter().filter(|n| n.starts_with("Call")).collect();
        assert_eq!(calls.len(), 90);
        assert!(
            calls.iter().all(|n| call_function(n).is_some()),
            "a Call* slot is missing"
        );
    }
}
