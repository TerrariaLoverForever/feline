// SPDX-License-Identifier: GPL-3.0-or-later
//! The whole computer's numbers: CPU time, memory in use, free disk space,
//! and a ping (ICMP echo) for the network test.

use std::path::Path;

use windows_sys::Win32::Foundation::{FILETIME, INVALID_HANDLE_VALUE};

use crate::wide;

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// The whole computer's CPU time since it started, in 100 ns units:
/// `(idle, total)`. Two readings give the CPU use in between.
pub fn cpu_times() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (zero, zero, zero);
    // SAFETY: three valid, writable FILETIMEs.
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    // Kernel time includes the idle time.
    (ok != 0).then(|| (ticks(idle), ticks(kernel) + ticks(user)))
}

/// `(total, available)` physical memory in bytes.
pub fn memory() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain numbers, so all-zero is valid;
    // Windows requires `dwLength` to be set.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a valid, writable MEMORYSTATUSEX with its length set.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some((status.ullTotalPhys, status.ullAvailPhys))
}

/// `(free, total)` bytes on the drive holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path = wide(path);
    let (mut free, mut total, mut all_free) = (0u64, 0u64, 0u64);
    // SAFETY: `path` is NUL-terminated; the three outputs are valid u64s.
    let ok = unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut total, &mut all_free) };
    (ok != 0).then_some((free, total))
}

/// Sends one ping to an IPv4 address. The round trip in milliseconds, or
/// `None` without a reply within `timeout_ms` (many servers ignore pings).
pub fn ping(address: [u8; 4], timeout_ms: u32) -> Option<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ICMP_ECHO_REPLY, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
    };
    // SAFETY: no arguments; the handle is closed below.
    let handle = unsafe { IcmpCreateFile() };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return None;
    }
    let data = *b"feline-ping";
    // Room for one reply, its data and an ICMP error message (as documented).
    let mut reply = vec![0u8; size_of::<ICMP_ECHO_REPLY>() + data.len() + 8 + 64];
    // The address is in network byte order, as it's laid out in memory.
    let destination = u32::from_ne_bytes(address);
    // SAFETY: `data` and `reply` are valid for the sizes given; the
    // options pointer may be null.
    let replies = unsafe {
        IcmpSendEcho(
            handle,
            destination,
            data.as_ptr().cast(),
            data.len() as u16,
            std::ptr::null(),
            reply.as_mut_ptr().cast(),
            reply.len() as u32,
            timeout_ms,
        )
    };
    let result = (replies > 0).then(|| {
        // SAFETY: on success the buffer starts with an ICMP_ECHO_REPLY;
        // read_unaligned because a Vec<u8> has no alignment guarantee.
        let echo: ICMP_ECHO_REPLY = unsafe { std::ptr::read_unaligned(reply.as_ptr().cast()) };
        // IP_SUCCESS = 0
        (echo.Status == 0).then_some(echo.RoundTripTime)
    });
    // SAFETY: closes the handle IcmpCreateFile gave.
    unsafe { IcmpCloseHandle(handle) };
    result.flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_computer_has_numbers() {
        let (idle, total) = cpu_times().unwrap();
        assert!(total >= idle && total > 0);
        let (total, available) = memory().unwrap();
        assert!(total > available && available > 0);
        let windir = std::env::var_os("WINDIR").unwrap();
        let (free, size) = disk_space(Path::new(&windir)).unwrap();
        assert!(size > free && free > 0);
    }

    #[test]
    fn this_computer_answers_its_own_ping() {
        assert!(ping([127, 0, 0, 1], 1000).is_some());
    }
}
