// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Roblox's in-app message bus (`com.roblox.universalapp.messagebus`): how
//! the app asks the platform things, e.g. "may I use the microphone?"
//! (`PermissionsProtocol`). On Android, Java code answers; here Feline does.
//!
//! Feline registers handlers through the engine's declared natives
//! (`MessageBus.doSubscribeProtocolMethodRequestRaw`,
//! `setRequestHandlerAsyncRaw`). The engine then calls `run` on Feline's
//! handler objects: `RawCallback.run(String message)` or
//! `RequestHandlerAsyncRaw.run(String message, String responseId)`. Feline
//! answers with `publishProtocolMethodResponseRaw`, and for the async form
//! also `callResponseHandlerRaw(responseId, body)`. Which methods exist and
//! how they're answered follows Mocktail
//! (<https://github.com/komaruworld/mocktail>, Apache-2.0,
//! src/runtime/roblox_permissions_bridge.cc and
//! roblox_message_bus_request_bridge.cc); the code is Feline's own.
//!
//! Unsafe: calling the engine's natives through their addresses.
#![allow(unsafe_code)]

use std::sync::Mutex;

use serde_json::{Value, json};

use crate::elf::Elf;
use crate::jni::{Handle, new_instance, new_string, string_value};
use crate::load::Image;

const BUS_CLASS: &str = "com/roblox/universalapp/messagebus/MessageBus";
const PERMISSIONS: &str = "PermissionsProtocol";
const MICROPHONE: &str = "MICROPHONE_ACCESS";
const LOCAL_NETWORK: &str = "LOCAL_NETWORK";

/// The engine's message bus natives (instance methods on MessageBus).
#[derive(Clone, Copy)]
struct Natives {
    subscribe_request: usize,
    set_async_handler: usize,
    publish_response: usize,
    call_response_handler: usize,
}

struct Registration {
    /// Feline's callback or handler object.
    object: usize,
    protocol: &'static str,
    method: &'static str,
}

struct Bus {
    natives: Natives,
    /// Feline's MessageBus object (the natives don't look at it).
    object: usize,
    registrations: Vec<Registration>,
}

static BUS: Mutex<Option<Bus>> = Mutex::new(None);

/// Finds the natives and registers Feline's handlers. Call once the app
/// has started (after `nativeAppBridgeV2StartAppWithParams`).
pub fn start(elf: &Elf, image: &Image, env: Handle) -> Result<(), String> {
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let find = |method: &str| {
        let name = crate::init::jni_symbol(BUS_CLASS, method);
        symbols
            .iter()
            .find(|s| s.defined && s.name == name)
            .map(|s| (image.bias() + s.value) as usize)
            .ok_or_else(|| format!("no {name}"))
    };
    let natives = Natives {
        subscribe_request: find("doSubscribeProtocolMethodRequestRaw")?,
        set_async_handler: find("setRequestHandlerAsyncRaw")?,
        publish_response: find("publishProtocolMethodResponseRaw")?,
        call_response_handler: find("callResponseHandlerRaw")?,
    };
    let bus = new_instance(BUS_CLASS) as usize;
    let methods = [
        "SupportsPermissions",
        "HasPermissions",
        "PermissionsRequest",
        "ShouldShowPermissionUpsell",
        "ShouldShowRequestPermissionRationale",
    ];
    // (method, legacy callback, async handler)
    let objects: Vec<(&'static str, usize, usize)> = methods
        .iter()
        .map(|&m| {
            (
                m,
                new_instance("com/roblox/universalapp/messagebus/RawCallback") as usize,
                new_instance("com/roblox/universalapp/messagebus/RequestHandlerAsyncRaw") as usize,
            )
        })
        .collect();
    // Known before registering: the engine may call them at once.
    *BUS.lock().unwrap_or_else(|p| p.into_inner()) = Some(Bus {
        natives,
        object: bus,
        registrations: objects
            .iter()
            .flat_map(|&(method, callback, handler)| {
                [callback, handler].map(|object| Registration {
                    object,
                    protocol: PERMISSIONS,
                    method,
                })
            })
            .collect(),
    });
    // SAFETY: the DEX signatures:
    // doSubscribeProtocolMethodRequestRaw(String, String, RawCallback, boolean) Connection
    // setRequestHandlerAsyncRaw(String, String, RequestHandlerAsyncRaw) void
    type Subscribe = extern "C" fn(Handle, Handle, Handle, Handle, Handle, u8) -> Handle;
    type SetHandler = extern "C" fn(Handle, Handle, Handle, Handle, Handle);
    let subscribe: Subscribe = unsafe { std::mem::transmute(natives.subscribe_request) };
    let set_handler: SetHandler = unsafe { std::mem::transmute(natives.set_async_handler) };
    for (method, callback, handler) in objects {
        let protocol = new_string(PERMISSIONS);
        let name = new_string(method);
        subscribe(env, bus as Handle, protocol, name, callback as Handle, 0);
        set_handler(env, bus as Handle, protocol, name, handler as Handle);
    }
    eprintln!("feline-runtime: message bus: {PERMISSIONS} answered by Feline (microphone allowed)");
    Ok(())
}

/// A Java call the engine made. If it's `run` on one of Feline's handler
/// objects, answers the request and returns true.
pub fn call(object: Handle, name: &str, signature: &str, read: &mut dyn FnMut() -> i64) -> bool {
    if name != "run" || object.is_null() {
        return false;
    }
    let (natives, bus, protocol, method) = {
        let guard = BUS.lock().unwrap_or_else(|p| p.into_inner());
        let Some(state) = guard.as_ref() else {
            return false;
        };
        let Some(entry) = state
            .registrations
            .iter()
            .find(|r| r.object == object as usize)
        else {
            return false;
        };
        (state.natives, state.object, entry.protocol, entry.method)
    };
    let message = string_value(read() as usize as Handle).unwrap_or_default();
    let response_id = if signature.starts_with("(Ljava/lang/String;Ljava/lang/String;)") {
        Some(read() as usize as Handle)
    } else {
        None
    };
    let (body, code) = answer(protocol, method, &message);
    eprintln!(
        "feline-runtime: message bus: {protocol}.{method} answered ({})",
        if response_id.is_some() { "async" } else { "legacy" }
    );
    let Some(env) = crate::jni::current_env() else {
        return true;
    };
    let body = body.to_string();
    // SAFETY: the DEX signatures:
    // publishProtocolMethodResponseRaw(String, String, String, int, String) void
    // callResponseHandlerRaw(String, String) void
    type Publish = extern "C" fn(Handle, Handle, Handle, Handle, Handle, i32, Handle);
    type Respond = extern "C" fn(Handle, Handle, Handle, Handle);
    let publish: Publish = unsafe { std::mem::transmute(natives.publish_response) };
    publish(
        env,
        bus as Handle,
        new_string(protocol),
        new_string(method),
        new_string(&body),
        code,
        new_string("{}"),
    );
    if let Some(id) = response_id {
        let respond: Respond = unsafe { std::mem::transmute(natives.call_response_handler) };
        respond(env, bus as Handle, id, new_string(&body));
    }
    true
}

/// The answer to a request: (JSON body, status code; 0 = fine).
fn answer(protocol: &str, method: &str, message: &str) -> (Value, i32) {
    if protocol != PERMISSIONS {
        return (json!({}), 13);
    }
    let asked: Vec<String> = serde_json::from_str::<Value>(message)
        .ok()
        .and_then(|v| v.get("permissions").cloned())
        .and_then(|p| serde_json::from_value(p).ok())
        .unwrap_or_default();
    match method {
        "SupportsPermissions" => (json!({ "permissions": [MICROPHONE, LOCAL_NETWORK] }), 0),
        // There's no permission dialog on the desktop: never ask to show one.
        "ShouldShowPermissionUpsell" | "ShouldShowRequestPermissionRationale" => (
            json!({ "upsellStatus": "HIDE", "hiddenUpsellPermissions": asked }),
            0,
        ),
        // HasPermissions, PermissionsRequest: the microphone (voice chat)
        // and the local network are allowed; anything else (camera,
        // contacts, …) isn't granted here.
        _ => {
            let missing: Vec<&String> = asked
                .iter()
                .filter(|p| p.as_str() != MICROPHONE && p.as_str() != LOCAL_NETWORK)
                .collect();
            let status = if missing.is_empty() { "AUTHORIZED" } else { "DENIED" };
            (json!({ "status": status, "missingPermissions": missing }), 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_is_allowed_and_dialogs_are_hidden() {
        let (body, code) = answer(PERMISSIONS, "HasPermissions", r#"{"permissions":["MICROPHONE_ACCESS"]}"#);
        assert_eq!(code, 0);
        assert_eq!(body["status"], "AUTHORIZED");
        let (body, _) = answer(
            PERMISSIONS,
            "PermissionsRequest",
            r#"{"permissions":["MICROPHONE_ACCESS","CAMERA_ACCESS"]}"#,
        );
        assert_eq!(body["status"], "DENIED");
        assert_eq!(body["missingPermissions"], json!(["CAMERA_ACCESS"]));
        let (body, _) = answer(PERMISSIONS, "ShouldShowPermissionUpsell", r#"{"permissions":["MICROPHONE_ACCESS"]}"#);
        assert_eq!(body["upsellStatus"], "HIDE");
        let (body, _) = answer(PERMISSIONS, "SupportsPermissions", "{}");
        assert_eq!(body["permissions"], json!([MICROPHONE, LOCAL_NETWORK]));
    }
}
