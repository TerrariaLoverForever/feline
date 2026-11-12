// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows calls around starting the game: checking that Roblox's program is
//! really Roblox's, the game's priority, which GPU it uses, and its CPU and
//! memory use while it runs.

use std::ffi::c_void;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, GetProcessTimes, HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, SetPriorityClass,
};

use crate::wide;

// ---------------------------------------------------------------------------
// Is it really Roblox?
// ---------------------------------------------------------------------------

/// Checks a program's digital signature (Authenticode, like Explorer's
/// "Digital Signatures" tab): it must be signed, unchanged since signing, and
/// chain to a certificate Windows trusts. Returns the signer's name, e.g.
/// `Roblox Corporation`; the caller decides whether that's the right one.
///
/// Revocation isn't checked online, so this works without network access
/// and never waits on a certificate server.
pub fn signer(path: &Path) -> Result<String, String> {
    let path_w = wide(path);
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path_w.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    // SAFETY: WINTRUST_DATA is plain data (numbers, pointers, a union of
    // pointers), so all-zero is a valid starting value.
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    // SAFETY: `data` points at `file`, which points at `path_w`; all three
    // live until the end of this function. No window handle: no UI.
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            (&raw mut data).cast::<c_void>(),
        )
    };
    let result = if status == 0 {
        // SAFETY: after a successful verify with STATEACTION_VERIFY,
        // `hWVTStateData` holds the provider data until STATEACTION_CLOSE.
        unsafe { signer_name(data.hWVTStateData) }.ok_or_else(|| "the signer couldn't be read".to_owned())
    } else {
        Err(match status as u32 {
            0x800B0100 => "it isn't digitally signed".to_owned(),
            0x80096010 => "its signature doesn't match: the file was changed after signing".to_owned(),
            0x800B0109 | 0x800B010A => "it's signed with a certificate Windows doesn't trust".to_owned(),
            0x800B0101 => "its signing certificate has expired".to_owned(),
            other => format!("Windows couldn't check its signature (error 0x{other:08X})"),
        })
    };
    // Release what the verify step allocated.
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: same structure as above; CLOSE frees `hWVTStateData`.
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            (&raw mut data).cast::<c_void>(),
        )
    };
    result
}

/// The display name of the first signer's certificate.
///
/// SAFETY: `state` must be the `hWVTStateData` of a successful WinVerifyTrust
/// call that hasn't been closed yet.
unsafe fn signer_name(state: HANDLE) -> Option<String> {
    // SAFETY: (for the whole block) every pointer comes from WinTrust's own
    // provider data, checked for null before use, and stays valid until the
    // caller closes the state.
    unsafe {
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return None;
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
        if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*signer).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let mut buffer = [0u16; 256];
        let len = CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            std::ptr::null(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        ) as usize;
        // `len` counts the terminating NUL.
        (len > 1).then(|| String::from_utf16_lossy(&buffer[..len - 1]))
    }
}

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

/// How much CPU time Windows gives the game compared with other programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Normal,
    AboveNormal,
    High,
}

/// Sets a running process's priority. False if Windows refused (e.g. the
/// process already ended).
pub fn set_priority(pid: u32, priority: Priority) -> bool {
    let class = match priority {
        Priority::Normal => NORMAL_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
    };
    // SAFETY: plain call; the handle is checked and closed below.
    let process = unsafe { OpenProcess(PROCESS_SET_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    // SAFETY: `process` is a valid handle with PROCESS_SET_INFORMATION.
    let ok = unsafe { SetPriorityClass(process, class) } != 0;
    // SAFETY: we own the handle.
    unsafe { CloseHandle(process) };
    ok
}

// ---------------------------------------------------------------------------
// Which GPU (laptops with two)
// ---------------------------------------------------------------------------

/// Where Windows keeps per-program GPU choices (Settings → System → Display
/// → Graphics writes the same values).
const GPU_KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
/// "High performance": the dedicated GPU.
const HIGH_PERFORMANCE: &str = "GpuPreference=2;";

/// The GPU choice Windows has for `exe`, e.g. `GpuPreference=2;`.
pub fn gpu_preference(exe: &Path) -> Option<String> {
    let key = wide(GPU_KEY);
    let name = wide(exe);
    let mut buffer = [0u16; 128];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: names are NUL-terminated; `buffer` is `size` writable bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
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
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// Asks Windows to run `exe` on the high-performance GPU (`true`), or removes
/// that choice again if Feline made it (`false`; any other choice the user
/// made in Windows' settings is left alone). Per user, no admin rights.
pub fn set_high_performance_gpu(exe: &Path, on: bool) -> Result<(), String> {
    let key = wide(GPU_KEY);
    let name = wide(exe);
    if on {
        if gpu_preference(exe).as_deref() == Some(HIGH_PERFORMANCE) {
            return Ok(());
        }
        let value = wide(HIGH_PERFORMANCE);
        // SAFETY: names are NUL-terminated; `value` is a NUL-terminated
        // UTF-16 string of the given byte length (including the NUL).
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        if status != 0 {
            return Err(format!("Windows refused the GPU choice (error {status})"));
        }
    } else if gpu_preference(exe).as_deref() == Some(HIGH_PERFORMANCE) {
        // SAFETY: names are NUL-terminated.
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-user settings in the registry
// ---------------------------------------------------------------------------

/// A text value under `HKEY_CURRENT_USER\<key>` (`name` None: the key's
/// default value).
pub fn user_string(key: &str, name: Option<&str>) -> Option<String> {
    let key = wide(key);
    let name = name.map(wide);
    let mut buffer = vec![0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: names are NUL-terminated (or null for the default value);
    // `buffer` is `size` writable bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
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
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// Sets a text value under `HKEY_CURRENT_USER\<key>` (creating the key).
pub fn set_user_string(key: &str, name: Option<&str>, value: &str) -> bool {
    let key = wide(key);
    let name = name.map(wide);
    let value = wide(value);
    // SAFETY: as in user_string; `value` is NUL-terminated UTF-16 of the
    // given byte length.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        ) == 0
    }
}

/// Removes a value under `HKEY_CURRENT_USER\<key>`.
pub fn delete_user_value(key: &str, name: &str) {
    let (key, name) = (wide(key), wide(name));
    // SAFETY: names are NUL-terminated.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
}

/// Removes `HKEY_CURRENT_USER\<key>` and everything in it.
pub fn delete_user_tree(key: &str) {
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;
    let key = wide(key);
    // SAFETY: a NUL-terminated name under the user's own hive.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Starts `command` when you log in (`None`: not any more), as "Feline".
pub fn run_at_login(command: Option<&str>) -> bool {
    match command {
        Some(command) => set_user_string(RUN_KEY, Some("Feline"), command),
        None => {
            delete_user_value(RUN_KEY, "Feline");
            true
        }
    }
}

/// Whether Feline starts when you log in.
pub fn runs_at_login() -> bool {
    user_string(RUN_KEY, Some("Feline")).is_some()
}

/// Tells Explorer that file associations changed (new icons, new handler).
pub fn associations_changed() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: a notification without items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
}

// ---------------------------------------------------------------------------
// A stuck game, and a friendly reminder
// ---------------------------------------------------------------------------

/// Ends every process whose program file is called `name` (e.g. a Roblox
/// that hangs in the background after its window closed). Returns how many
/// were ended.
pub fn end_processes(name: &str) -> usize {
    use windows_sys::Win32::System::Threading::{PROCESS_TERMINATE, TerminateProcess};
    crate::processes()
        .iter()
        .filter(|p| p.exe_name.eq_ignore_ascii_case(name))
        .filter(|p| {
            // SAFETY: plain call; the handle is checked and closed below.
            let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, p.pid) };
            if process.is_null() {
                return false;
            }
            // SAFETY: `process` is valid with PROCESS_TERMINATE.
            let ok = unsafe { TerminateProcess(process, 1) } != 0;
            // SAFETY: we own the handle.
            unsafe { CloseHandle(process) };
            ok
        })
        .count()
}

/// Ends one process (the Roblox Feline started). Returns whether it ended.
pub fn end_process(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: plain call; the handle is checked and closed below.
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if process.is_null() {
        return false;
    }
    // SAFETY: `process` is valid with PROCESS_TERMINATE.
    let ok = unsafe { TerminateProcess(process, 0) } != 0;
    // SAFETY: we own the handle.
    unsafe { CloseHandle(process) };
    ok
}

/// Shows a small message box on top of other windows, without blocking the
/// caller (it runs on its own thread until it's closed).
pub fn notify(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
    };
    let (title, text) = (wide(title), wide(text));
    let _ = std::thread::Builder::new()
        .name("feline-notify".into())
        .spawn(move || {
            // SAFETY: both strings are NUL-terminated and live on this
            // thread until the box is closed; no owner window.
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
                )
            };
        });
}

/// Whether a process is still running (not only still known to Windows).
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::STILL_ACTIVE;
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;
    // SAFETY: plain call; the handle is checked and closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    let mut code = 0u32;
    // SAFETY: `process` is valid with query rights; `code` is writable.
    let ok = unsafe { GetExitCodeProcess(process, &mut code) } != 0;
    // SAFETY: we own the handle.
    unsafe { CloseHandle(process) };
    ok && code == STILL_ACTIVE as u32
}

// ---------------------------------------------------------------------------
// More than one Roblox at a time
// ---------------------------------------------------------------------------

/// Roblox's "only one of me" markers, held by Feline: while another program
/// holds them, no Roblox becomes the one that closes the others, so
/// several Roblox windows can run side by side (what Bloxstrap's forks call
/// multi-instance). Released when dropped.
pub struct RobloxSingleton {
    handles: Vec<HANDLE>,
}

impl RobloxSingleton {
    /// Takes the markers. `Err` if a Roblox already has them (it was
    /// started before Feline could), or Windows refused.
    pub fn hold() -> Result<Self, String> {
        use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW};
        let mut held = Self { handles: Vec::new() };
        let mutex = wide("ROBLOX_singletonMutex");
        // SAFETY: a NUL-terminated name; the handle is checked and kept.
        let (handle, existed) =
            unsafe { (CreateMutexW(std::ptr::null(), 1, mutex.as_ptr()), GetLastError() == ERROR_ALREADY_EXISTS) };
        if handle.is_null() {
            return Err("Windows didn't let Feline hold Roblox's marker".to_owned());
        }
        held.handles.push(handle);
        if existed {
            return Err("Roblox was already running before Feline could make room for more".to_owned());
        }
        let event = wide("ROBLOX_singletonEvent");
        // SAFETY: as above (a manual-reset event, not signalled).
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, event.as_ptr()) };
        if !handle.is_null() {
            held.handles.push(handle);
        }
        Ok(held)
    }
}

impl Drop for RobloxSingleton {
    fn drop(&mut self) {
        for &h in &self.handles {
            // SAFETY: handles we own and haven't closed.
            unsafe { CloseHandle(h) };
        }
    }
}

/// A named marker of Feline's own, so only one Feline helper of a kind runs
/// at a time. `None` if another one already holds it.
pub struct OnlyOne(HANDLE);

impl OnlyOne {
    pub fn take(name: &str) -> Option<Self> {
        use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name = wide(format!("Local\\Feline{name}"));
        // SAFETY: a NUL-terminated name; the handle is checked.
        let (handle, existed) =
            unsafe { (CreateMutexW(std::ptr::null(), 0, name.as_ptr()), GetLastError() == ERROR_ALREADY_EXISTS) };
        if handle.is_null() {
            return None;
        }
        if existed {
            // SAFETY: our handle to the existing marker.
            unsafe { CloseHandle(handle) };
            return None;
        }
        Some(Self(handle))
    }

    /// Whether some process holds the marker `name` right now.
    pub fn held(name: &str) -> bool {
        Self::take(name).is_none()
    }
}

impl Drop for OnlyOne {
    fn drop(&mut self) {
        // SAFETY: a handle we own.
        unsafe { CloseHandle(self.0) };
    }
}

// ---------------------------------------------------------------------------
// CPU and memory of the running game
// ---------------------------------------------------------------------------

/// A process's CPU time so far (user + kernel) and its memory in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// In 100-nanosecond units, as Windows counts.
    pub cpu_time: u64,
    /// Working set: the memory the process currently has in RAM, in bytes.
    pub memory: u64,
}

/// `None` if the process can't be opened (it ended, or isn't ours).
pub fn usage(pid: u32) -> Option<Usage> {
    // SAFETY: plain call; the handle is checked and closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: `process` is valid with query rights; the four FILETIMEs are
    // writable.
    let times_ok =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
    // SAFETY: PROCESS_MEMORY_COUNTERS is plain numbers; `cb` is set.
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: `memory` is writable and `cb` bytes long.
    let memory_ok = unsafe { GetProcessMemoryInfo(process, &mut memory, memory.cb) } != 0;
    // SAFETY: we own the handle.
    unsafe { CloseHandle(process) };
    if !times_ok || !memory_ok {
        return None;
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Some(Usage {
        cpu_time: ticks(kernel) + ticks(user),
        memory: memory.WorkingSetSize as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roblox_is_signed_by_roblox_if_installed() {
        // Windows' own programs are signed through catalogs, which this check
        // doesn't cover; Roblox's player carries its signature inside.
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let versions = Path::new(&local).join("Roblox").join("Versions");
        let Some(exe) = std::fs::read_dir(versions)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("RobloxPlayerBeta.exe"))
            .find(|p| p.is_file())
        else {
            return;
        };
        assert_eq!(signer(&exe).as_deref(), Ok("Roblox Corporation"));
    }

    #[test]
    fn unsigned_files_are_refused() {
        let file = std::env::temp_dir().join("feline-unsigned-test.exe");
        std::fs::write(&file, b"MZ not really a program").unwrap();
        assert!(signer(&file).is_err());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn own_usage_is_readable() {
        let usage = usage(std::process::id()).expect("readable");
        assert!(usage.memory > 0);
    }
}
