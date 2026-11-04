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

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Mutex, OnceLock};

use crate::jni_names::{JAVA_VM, JNI_ENV};
use crate::stubs::Stubs;

type JInt = i32;
type JBoolean = u8;
type Handle = *mut c_void;

pub const JNI_VERSION_1_6: JInt = 0x0001_0006;
const JNI_OK: JInt = 0;

/// One object in the fake Java world.
enum Object {
    Class(String),
    Str(CString),
}

#[derive(Default)]
struct World {
    objects: Vec<Object>,
    /// (class, name, signature, static?) per method or field ID.
    members: Vec<(String, String, String, bool)>,
    /// (class, method name, signature) of each registered native.
    natives: Vec<(String, String, String)>,
}

static WORLD: Mutex<World> = Mutex::new(World {
    objects: Vec::new(),
    members: Vec::new(),
    natives: Vec::new(),
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
    handle(world.objects.len() - 1)
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

fn log(what: String) {
    eprintln!("feline-runtime: jni: {what}");
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
    log(format!("FindClass {name}"));
    add_object(Object::Class(name))
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
    log(format!("Get{kind} {label}{class}.{name} {sig}"));
    let mut world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    world.members.push((class, name, sig, is_static));
    handle(world.members.len() - 1)
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
        world.natives.push((class.clone(), name, sig));
    }
    log(format!("RegisterNatives {class}: {count} natives"));
    JNI_OK
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
    if let Some(i) = world
        .objects
        .iter()
        .position(|o| matches!(o, Object::Class(n) if n == name))
    {
        return handle(i);
    }
    world.objects.push(Object::Class(name.to_owned()));
    handle(world.objects.len() - 1)
}

/// `GetObjectClass`: Feline's objects are classes or strings.
extern "C" fn get_object_class(_env: Handle, object: Handle) -> Handle {
    trace("GetObjectClass");
    let name = {
        let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
        match index(object).and_then(|i| world.objects.get(i)) {
            Some(Object::Class(_)) => "java/lang/Class",
            Some(Object::Str(_)) => "java/lang/String",
            None => "java/lang/Object",
        }
    };
    log(format!("GetObjectClass {object:p} -> {name}"));
    class_handle(name)
}

/// `GetSuperclass`: every Feline class extends java/lang/Object, which has
/// no superclass.
extern "C" fn get_superclass(_env: Handle, class: Handle) -> Handle {
    trace("GetSuperclass");
    let name = class_name(class);
    if name == "java/lang/Object" {
        return std::ptr::null_mut();
    }
    log(format!("GetSuperclass {name} -> java/lang/Object"));
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
    log(format!("IsInstanceOf {actual} of {wanted} -> {result}"));
    JBoolean::from(result)
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

/// Logs a placeholder answer (first 3 calls per method) and returns the
/// method's return type.
fn answer(method: Handle, kind: &str, value: &str) -> String {
    use std::collections::HashMap;
    static SEEN: Mutex<Option<HashMap<usize, u32>>> = Mutex::new(None);
    let (description, ret) = {
        let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
        match index(method).and_then(|i| world.members.get(i)) {
            Some((class, name, sig, is_static)) => (
                format!(
                    "{}{class}.{name} {sig}",
                    if *is_static { "static " } else { "" }
                ),
                return_type(sig).to_owned(),
            ),
            None => (format!("<unknown method {method:p}>"), "V".to_owned()),
        }
    };
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    let count = seen
        .get_or_insert_with(HashMap::new)
        .entry(method as usize)
        .or_insert(0);
    *count += 1;
    if *count <= 3 {
        log(format!(
            "{kind} {description} -> {value} (placeholder; no Java code runs)"
        ));
    }
    ret
}

/// The placeholder for an object-returning method: an empty String for
/// `Ljava/lang/String;`, otherwise null.
fn object_answer(method: Handle, kind: &str) -> Handle {
    let ret = {
        let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
        index(method)
            .and_then(|i| world.members.get(i))
            .map(|(_, _, sig, _)| return_type(sig).to_owned())
            .unwrap_or_default()
    };
    if ret == "Ljava/lang/String;" {
        answer(method, kind, "\"\"");
        add_object(Object::Str(CString::default()))
    } else {
        answer(method, kind, "null");
        std::ptr::null_mut()
    }
}

/// One family: `$v` (…), `$vv` (va_list) and `$va` (jvalue array) for an
/// instance (`obj`), static (`class`) or non-virtual (`obj, class`) call.
macro_rules! calls {
    ($kind:ident, $ty:ty, $zero:expr, $label:literal, $m:ident, $mv:ident, $ma:ident, $s:ident, $sv:ident, $sa:ident, $n:ident, $nv:ident, $na:ident) => {
        unsafe extern "C" fn $m(_env: Handle, _obj: Handle, method: Handle, _args: ...) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $mv(_env: Handle, _obj: Handle, method: Handle, _args: *mut c_void) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $ma(_env: Handle, _obj: Handle, method: Handle, _args: *const c_void) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $s(_env: Handle, _class: Handle, method: Handle, _args: ...) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $sv(_env: Handle, _class: Handle, method: Handle, _args: *mut c_void) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $sa(_env: Handle, _class: Handle, method: Handle, _args: *const c_void) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $n(_env: Handle, _obj: Handle, _class: Handle, method: Handle, _args: ...) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $nv(_env: Handle, _obj: Handle, _class: Handle, method: Handle, _args: *mut c_void) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
        unsafe extern "C" fn $na(_env: Handle, _obj: Handle, _class: Handle, method: Handle, _args: *const c_void) -> $ty {
            calls!(@body $kind, method, $label, $zero)
        }
    };
    (@body object, $method:ident, $label:literal, $zero:expr) => {{
        trace("Call…ObjectMethod");
        object_answer($method, "call")
    }};
    (@body value, $method:ident, $label:literal, $zero:expr) => {{
        trace(concat!("Call…", $label, "Method"));
        answer($method, "call", &format!("{:?}", $zero));
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
// The tables
// ---------------------------------------------------------------------------

/// Feline's implementation for a slot, by its JNI name.
fn implemented(name: &str) -> Option<usize> {
    if let Some(address) = call_function(name) {
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
            Object::Str(_) => None,
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
