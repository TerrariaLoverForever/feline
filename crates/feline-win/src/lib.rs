// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's Windows helpers: the few Windows API calls Feline needs, behind
//! safe functions. `feline-core` forbids `unsafe` code, so the calls live
//! here, each with a note on why it's sound.
//!
//! On other systems this crate is empty.

#![cfg(windows)]
// The workspace denies `unsafe`; this crate exists to hold the calls that
// need it, so it allows it, and nothing else does.
#![allow(unsafe_code)]

pub mod game;
pub mod overlay;
pub mod stats;
pub mod tray;
pub mod window;

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
    TH32CS_SNAPPROCESS,
};

/// One running process: its ID and its program file's name, e.g.
/// `RobloxPlayerBeta.exe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub exe_name: String,
}

/// Every running process (of every user), from a ToolHelp snapshot. Empty if
/// the snapshot can't be taken.
pub fn processes() -> Vec<ProcessInfo> {
    let mut list = Vec::new();
    // SAFETY: plain call with constant flags; the result is checked below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        return list;
    }
    // SAFETY: PROCESSENTRY32W is plain data (numbers and a u16 array), so
    // all-zero is a valid value. Windows requires `dwSize` to be set.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snapshot` is a valid snapshot handle and `entry` a valid,
    // writable PROCESSENTRY32W with its size set.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let name = &entry.szExeFile;
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        list.push(ProcessInfo {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            exe_name: String::from_utf16_lossy(&name[..len]),
        });
        // SAFETY: as for Process32FirstW.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: `snapshot` is a handle we own and haven't closed.
    unsafe { CloseHandle(snapshot) };
    list
}

/// Connects this program's text output to the console of the program that
/// started it (a Command Prompt or PowerShell window), if there is one.
///
/// Feline is built as a *windowed* program on Windows, so opening it from
/// the Start menu or a link doesn't flash a console window. Text commands
/// (`feline --help`) call this to still print in the terminal they were
/// typed in. Returns false when there is no such console.
pub fn attach_parent_console() -> bool {
    // SAFETY: plain call with a constant argument; failure is reported by
    // the return value and leaves the process unchanged.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
}

/// Removes the user's saved logins in Windows' Credential Manager whose
/// name starts with `prefix` (e.g. Roblox Studio's). Returns how many.
pub fn forget_credentials(prefix: &str) -> usize {
    use windows_sys::Win32::Security::Credentials::{CREDENTIALW, CredDeleteW, CredEnumerateW, CredFree};
    let filter = wide(format!("{prefix}*"));
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `filter` is NUL-terminated; on success Windows gives an array
    // of `count` credential pointers, freed with CredFree below.
    if unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut list) } == 0 {
        return 0;
    }
    let mut removed = 0;
    for i in 0..count as usize {
        // SAFETY: `i < count`, so the pointer is one Windows gave us.
        let cred = unsafe { &**list.add(i) };
        // SAFETY: a credential's target name and type are valid while the
        // list hasn't been freed.
        if unsafe { CredDeleteW(cred.TargetName, cred.Type, 0) } != 0 {
            removed += 1;
        }
    }
    // SAFETY: frees the array CredEnumerateW allocated.
    unsafe { CredFree(list.cast()) };
    removed
}

/// This computer's offset from UTC right now, in seconds (east positive),
/// daylight saving time included.
pub fn utc_offset_seconds() -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    // SAFETY: TIME_ZONE_INFORMATION is plain data; all-zero is valid.
    let mut info: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a valid, writable structure.
    let which = unsafe { GetTimeZoneInformation(&mut info) };
    // 2 = daylight saving time in effect; 1 = standard; 0 = no such rules.
    let extra = match which {
        2 => info.DaylightBias,
        1 => info.StandardBias,
        _ => 0,
    };
    -i64::from(info.Bias + extra) * 60
}

/// Total physical memory in bytes.
pub fn total_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain numbers, so all-zero is valid;
    // Windows requires `dwLength` to be set.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a valid, writable MEMORYSTATUSEX with its length set.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status.ullTotalPhys)
}

/// A string value under `HKEY_LOCAL_MACHINE`, e.g.
/// `machine_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")`.
pub fn machine_string(subkey: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let subkey = wide(subkey);
    let value = wide(value);
    let mut size: u32 = 0;
    // SAFETY: both names are NUL-terminated; a null buffer asks for the size.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` is writable and `size` bytes long.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..len]).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// A number (DWORD) value under `HKEY_LOCAL_MACHINE`.
pub fn machine_number(subkey: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    let subkey = wide(subkey);
    let value = wide(value);
    let mut number: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: both names are NUL-terminated; `number` is 4 writable bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut number).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(number)
}

/// Whether Windows is set to light mode for apps (Settings → Personalization
/// → Colors). `None` if Windows doesn't say (older versions).
pub fn apps_use_light_theme() -> Option<bool> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("AppsUseLightTheme");
    let mut number: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: both names are NUL-terminated; `number` is 4 writable bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut number).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(number != 0)
}

/// A NUL-terminated UTF-16 copy of `text`, for the "W" Windows functions.
pub(crate) fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

/// The `FileVersion` text of a program's version resource (what Explorer
/// shows under Properties → Details), e.g. `0, 746, 0, 7461267` for Roblox,
/// tidied to `0.746.0.7461267`. `None` if the file has none.
///
/// The text is used rather than the binary version numbers because those are
/// 16 bits each, and Roblox's build number is bigger than that.
pub fn file_version(path: &Path) -> Option<String> {
    let path = wide(path);
    // SAFETY: `path` is NUL-terminated; the second argument may be null.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` is writable and exactly `size` bytes long.
    let ok = unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) };
    if ok == 0 {
        return None;
    }
    // Which language/code page the strings are stored under.
    let translation = query(&data, r"\VarFileInfo\Translation")?;
    let [a, b, c, d, ..] = *translation else {
        return None;
    };
    let language = u16::from_le_bytes([a, b]);
    let code_page = u16::from_le_bytes([c, d]);
    let key = format!(r"\StringFileInfo\{language:04x}{code_page:04x}\FileVersion");
    let text = query(&data, &key)?;
    let units: Vec<u16> = text
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&u| u != 0)
        .collect();
    let version = String::from_utf16_lossy(&units).replace(", ", ".").replace(',', ".");
    let version = version.trim().to_owned();
    (!version.is_empty()).then_some(version)
}

/// One value of a version resource read by [`GetFileVersionInfoW`], as bytes
/// (borrowed from `data`).
fn query<'a>(data: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let key = wide(key);
    let mut value: *mut c_void = std::ptr::null_mut();
    let mut len: u32 = 0;
    // SAFETY: `data` holds a version resource from GetFileVersionInfoW and
    // `key` is NUL-terminated. On success, Windows points `value` into
    // `data` (not a copy), with `len` units after it.
    let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut value, &mut len) };
    if ok == 0 || value.is_null() {
        return None;
    }
    let start = (value as usize).checked_sub(data.as_ptr() as usize)?;
    // `len` counts characters for strings and bytes for binary values; the
    // two-bytes-per-character case is the longer one, capped at `data`'s end.
    let end = start.checked_add(len as usize * 2)?.min(data.len());
    data.get(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_without_a_version_have_none() {
        let not_a_program = std::env::temp_dir().join("feline-no-version.txt");
        std::fs::write(&not_a_program, "x").unwrap();
        assert_eq!(file_version(&not_a_program), None);
        let _ = std::fs::remove_file(&not_a_program);
    }

    #[test]
    fn registry_and_memory_can_be_read() {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        assert!(machine_string(key, "CurrentBuild").is_some());
        assert_eq!(machine_string(key, "FelineNoSuchValue"), None);
        assert!(total_memory().unwrap() > 512 * 1024 * 1024);
    }

    #[test]
    fn windows_programs_have_a_version() {
        let windir = std::env::var_os("WINDIR").expect("WINDIR");
        let notepad = Path::new(&windir).join("System32").join("notepad.exe");
        let version = file_version(&notepad).expect("notepad has a version");
        assert!(version.chars().next().unwrap().is_ascii_digit(), "{version}");
    }

    #[test]
    fn this_process_is_listed() {
        let me = std::process::id();
        let list = processes();
        let found = list.iter().find(|p| p.pid == me).expect("listed");
        assert!(found.exe_name.to_ascii_lowercase().ends_with(".exe"));
    }
}
