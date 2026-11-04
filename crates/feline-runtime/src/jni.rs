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

// ---------------------------------------------------------------------------
// JNIEnv functions Feline implements
// ---------------------------------------------------------------------------

extern "C" fn get_version(_env: Handle) -> JInt {
    JNI_VERSION_1_6
}

unsafe extern "C" fn find_class(_env: Handle, name: *const c_char) -> Handle {
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
    JNI_OK
}

// No Java code runs, so no Java exception is ever pending.
extern "C" fn exception_check(_env: Handle) -> JBoolean {
    0
}
extern "C" fn exception_occurred(_env: Handle) -> Handle {
    std::ptr::null_mut()
}
extern "C" fn exception_clear(_env: Handle) {}

// References: Feline's objects live as long as the process, so a new
// reference is the same handle and deleting one does nothing.
extern "C" fn new_ref(_env: Handle, object: Handle) -> Handle {
    object
}
extern "C" fn delete_ref(_env: Handle, _object: Handle) {}
extern "C" fn push_local_frame(_env: Handle, _capacity: JInt) -> JInt {
    JNI_OK
}
extern "C" fn pop_local_frame(_env: Handle, result: Handle) -> Handle {
    result
}
extern "C" fn ensure_local_capacity(_env: Handle, _capacity: JInt) -> JInt {
    JNI_OK
}
extern "C" fn is_same_object(_env: Handle, a: Handle, b: Handle) -> JBoolean {
    JBoolean::from(a == b)
}
/// JNILocalRefType: every Feline handle behaves like a valid reference.
extern "C" fn get_object_ref_type(_env: Handle, _object: Handle) -> JInt {
    1
}

unsafe extern "C" fn new_string_utf(_env: Handle, s: *const c_char) -> Handle {
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
extern "C" fn release_string_utf_chars(_env: Handle, _string: Handle, _chars: *const c_char) {}
extern "C" fn get_string_utf_length(_env: Handle, string: Handle) -> JInt {
    let world = WORLD.lock().unwrap_or_else(|p| p.into_inner());
    match index(string).and_then(|i| world.objects.get(i)) {
        Some(Object::Str(s)) => s.as_bytes().len() as JInt,
        _ => 0,
    }
}

unsafe extern "C" fn get_java_vm(_env: Handle, vm: *mut Handle) -> JInt {
    // SAFETY: the engine passes a writable JavaVM* slot.
    unsafe { *vm = tables().vm };
    JNI_OK
}

// ---------------------------------------------------------------------------
// JavaVM functions
// ---------------------------------------------------------------------------

/// GetEnv, AttachCurrentThread(AsDaemon): every thread gets the same env;
/// Feline's functions keep no per-thread state.
unsafe extern "C" fn vm_get_env(_vm: Handle, env: *mut Handle, _arg: *mut c_void) -> JInt {
    // SAFETY: the engine passes a writable JNIEnv* slot.
    unsafe { *env = tables().env };
    JNI_OK
}
extern "C" fn vm_detach(_vm: Handle) -> JInt {
    JNI_OK
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// Feline's implementation for a slot, by its JNI name.
fn implemented(name: &str) -> Option<usize> {
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
        "GetJavaVM" => get_java_vm as unsafe extern "C" fn(Handle, *mut Handle) -> JInt as usize,
        _ => return None,
    })
}

struct Tables {
    /// The `JNIEnv*` / `JavaVM*` values handed to the engine: each points to
    /// a pointer to its function table.
    env: Handle,
    vm: Handle,
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
    let env_ptr: &'static mut *const usize = Box::leak(Box::new(Box::leak(env_table).as_ptr()));
    let vm_ptr: &'static mut *const usize = Box::leak(Box::new(Box::leak(vm_table).as_ptr()));
    Ok(Tables {
        env: (env_ptr as *mut *const usize).cast(),
        vm: (vm_ptr as *mut *const usize).cast(),
    })
}

fn vm_implemented(name: &str) -> Option<usize> {
    type GetEnv = unsafe extern "C" fn(Handle, *mut Handle, *mut c_void) -> JInt;
    Some(match name {
        "GetEnv" | "AttachCurrentThread" | "AttachCurrentThreadAsDaemon" => {
            vm_get_env as GetEnv as usize
        }
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
    }
}
