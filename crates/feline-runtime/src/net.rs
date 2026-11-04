// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Name lookups (`getaddrinfo` and friends) for the engine, on glibc's.
//!
//! glibc does the work (DNS, /etc/hosts, nsswitch), but bionic's interface
//! differs in three ways (AOSP `libc/include/netdb.h`, compared with glibc's
//! header on the target system):
//!
//! - `struct addrinfo` has `ai_canonname` **before** `ai_addr`; glibc has
//!   them the other way round. Results are copied into bionic-shaped nodes.
//! - The `AI_*` and `NI_*` flag bits are BSD's, not glibc's.
//! - The `EAI_*` error codes are BSD's positive numbers; glibc's are
//!   negative and numbered differently.
//!
//! Socket addresses (`sockaddr_in`, `sockaddr_in6`) are the kernel's layout
//! in both, so they're copied unchanged.
//!
//! # `unsafe` in this module
//!
//! The functions take the engine's C pointers and call glibc's.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int};

/// bionic's `struct addrinfo` (field order as in AOSP's netdb.h).
#[repr(C)]
pub struct BionicAddrinfo {
    ai_flags: c_int,
    ai_family: c_int,
    ai_socktype: c_int,
    ai_protocol: c_int,
    ai_addrlen: libc::socklen_t,
    ai_canonname: *mut c_char,
    ai_addr: *mut libc::sockaddr,
    ai_next: *mut BionicAddrinfo,
}

/// (bionic bit, glibc bit) for `AI_*`.
const AI_FLAGS: &[(c_int, c_int)] = &[
    (0x1, libc::AI_PASSIVE),
    (0x2, libc::AI_CANONNAME),
    (0x4, libc::AI_NUMERICHOST),
    (0x8, libc::AI_NUMERICSERV),
    (0x100, libc::AI_ALL),
    (0x400, libc::AI_ADDRCONFIG),
    (0x800, libc::AI_V4MAPPED),
    // AI_V4MAPPED_CFG (0x200) has no glibc equivalent and is dropped.
];

/// (bionic bit, glibc bit) for `NI_*`.
const NI_FLAGS: &[(c_int, c_int)] = &[
    (0x1, libc::NI_NOFQDN),
    (0x2, libc::NI_NUMERICHOST),
    (0x4, libc::NI_NAMEREQD),
    (0x8, libc::NI_NUMERICSERV),
    (0x10, libc::NI_DGRAM),
];

/// (bionic code, glibc code) for `EAI_*`.
const EAI_CODES: &[(c_int, c_int)] = &[
    (1, -9), // EAI_ADDRFAMILY
    (2, libc::EAI_AGAIN),
    (3, libc::EAI_BADFLAGS),
    (4, libc::EAI_FAIL),
    (5, libc::EAI_FAMILY),
    (6, libc::EAI_MEMORY),
    (7, libc::EAI_NODATA),
    (8, libc::EAI_NONAME),
    (9, libc::EAI_SERVICE),
    (10, libc::EAI_SOCKTYPE),
    (11, libc::EAI_SYSTEM),
    (14, libc::EAI_OVERFLOW),
];

fn map_bits(value: c_int, table: &[(c_int, c_int)], to_glibc: bool) -> c_int {
    table.iter().fold(0, |out, &(bionic, glibc)| {
        let (from, to) = if to_glibc {
            (bionic, glibc)
        } else {
            (glibc, bionic)
        };
        if value & from != 0 { out | to } else { out }
    })
}

/// A glibc error code as bionic's (0 stays 0; unknown ones become EAI_FAIL).
fn bionic_error(glibc: c_int) -> c_int {
    if glibc == 0 {
        return 0;
    }
    EAI_CODES
        .iter()
        .find(|(_, g)| *g == glibc)
        .map_or(4, |(b, _)| *b)
}

/// Logs the first lookups (host name and glibc's result), so it's visible
/// what the engine connects to.
fn log_lookup(node: *const c_char, error: c_int) {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 30 {
        return;
    }
    let name = if node.is_null() {
        "<none>".into()
    } else {
        // SAFETY: the engine's NUL-terminated host name.
        unsafe { std::ffi::CStr::from_ptr(node) }.to_string_lossy()
    };
    let result = if error == 0 {
        "found".to_owned()
    } else {
        // SAFETY: glibc returns a static string.
        unsafe { std::ffi::CStr::from_ptr(libc::gai_strerror(error)) }
            .to_string_lossy()
            .into_owned()
    };
    eprintln!("feline-runtime: looked up {name}: {result}");
}

/// `int getaddrinfo(node, service, hints, res)` with bionic's structures.
///
/// # Safety
/// As C's: valid strings or NULL, `hints` NULL or a bionic addrinfo,
/// `res` writable.
pub unsafe extern "C" fn getaddrinfo(
    node: *const c_char,
    service: *const c_char,
    hints: *const BionicAddrinfo,
    res: *mut *mut BionicAddrinfo,
) -> c_int {
    // SAFETY: an all-zero addrinfo is glibc's "no hints" value.
    let mut glibc_hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    if !hints.is_null() {
        // SAFETY: the caller's hints; only the first four ints are inputs.
        let h = unsafe { &*hints };
        glibc_hints.ai_flags = map_bits(h.ai_flags, AI_FLAGS, true);
        glibc_hints.ai_family = h.ai_family;
        glibc_hints.ai_socktype = h.ai_socktype;
        glibc_hints.ai_protocol = h.ai_protocol;
    }
    let mut list: *mut libc::addrinfo = std::ptr::null_mut();
    // SAFETY: valid inputs as above; glibc allocates `list`.
    let error = unsafe {
        libc::getaddrinfo(
            node,
            service,
            if hints.is_null() {
                std::ptr::null()
            } else {
                &glibc_hints
            },
            &mut list,
        )
    };
    log_lookup(node, error);
    if error != 0 {
        return bionic_error(error);
    }
    // Copy into bionic-shaped nodes: one malloc per node holding the node,
    // its address and its name, so `freeaddrinfo` frees one block each.
    let mut head: *mut BionicAddrinfo = std::ptr::null_mut();
    let mut tail: *mut *mut BionicAddrinfo = &mut head;
    let mut at = list;
    while !at.is_null() {
        // SAFETY: a node of glibc's list.
        let g = unsafe { &*at };
        let addr_len = g.ai_addrlen as usize;
        let name_len = if g.ai_canonname.is_null() {
            0
        } else {
            // SAFETY: glibc's NUL-terminated name.
            unsafe { libc::strlen(g.ai_canonname) + 1 }
        };
        let node_size = std::mem::size_of::<BionicAddrinfo>();
        // SAFETY: plain malloc; checked for NULL.
        let block = unsafe { libc::malloc(node_size + addr_len + name_len) }.cast::<u8>();
        if block.is_null() {
            // SAFETY: both lists are well-formed.
            unsafe {
                freeaddrinfo(head);
                libc::freeaddrinfo(list);
            }
            return 6; // EAI_MEMORY
        }
        // SAFETY: `block` has room for the node, address and name.
        unsafe {
            let addr = block.add(node_size);
            let name = addr.add(addr_len);
            if !g.ai_addr.is_null() {
                std::ptr::copy_nonoverlapping(g.ai_addr.cast::<u8>(), addr, addr_len);
            }
            if name_len > 0 {
                std::ptr::copy_nonoverlapping(g.ai_canonname.cast::<u8>(), name, name_len);
            }
            let out = block.cast::<BionicAddrinfo>();
            out.write(BionicAddrinfo {
                ai_flags: map_bits(g.ai_flags, AI_FLAGS, false),
                ai_family: g.ai_family,
                ai_socktype: g.ai_socktype,
                ai_protocol: g.ai_protocol,
                ai_addrlen: g.ai_addrlen,
                ai_canonname: if name_len > 0 {
                    name.cast()
                } else {
                    std::ptr::null_mut()
                },
                ai_addr: if g.ai_addr.is_null() {
                    std::ptr::null_mut()
                } else {
                    addr.cast()
                },
                ai_next: std::ptr::null_mut(),
            });
            *tail = out;
            tail = &mut (*out).ai_next;
        }
        at = g.ai_next;
    }
    // SAFETY: glibc's own list, no longer used.
    unsafe {
        libc::freeaddrinfo(list);
        *res = head;
    }
    0
}

/// `void freeaddrinfo(struct addrinfo*)` for lists from [`getaddrinfo`].
///
/// # Safety
/// `list` came from Feline's `getaddrinfo` (or is NULL).
pub unsafe extern "C" fn freeaddrinfo(mut list: *mut BionicAddrinfo) {
    while !list.is_null() {
        // SAFETY: each node is one malloc block.
        unsafe {
            let next = (*list).ai_next;
            libc::free(list.cast());
            list = next;
        }
    }
}

/// `const char* gai_strerror(int)` for bionic's codes.
pub extern "C" fn gai_strerror(code: c_int) -> *const c_char {
    let glibc = EAI_CODES
        .iter()
        .find(|(b, _)| *b == code)
        .map_or(libc::EAI_FAIL, |(_, g)| *g);
    // SAFETY: glibc returns a static string.
    unsafe { libc::gai_strerror(glibc) }
}

/// `int getnameinfo(sa, salen, host, hostlen, serv, servlen, flags)`;
/// bionic's lengths are `size_t`, glibc's `socklen_t` (both are far below
/// 4 GiB in practice; larger ones are clamped).
///
/// # Safety
/// As C's.
pub unsafe extern "C" fn getnameinfo(
    address: *const libc::sockaddr,
    address_len: libc::socklen_t,
    host: *mut c_char,
    host_len: usize,
    service: *mut c_char,
    service_len: usize,
    flags: c_int,
) -> c_int {
    let clamp = |n: usize| libc::socklen_t::try_from(n).unwrap_or(libc::socklen_t::MAX);
    // SAFETY: the caller's buffers, with their lengths.
    let error = unsafe {
        libc::getnameinfo(
            address,
            address_len,
            host,
            clamp(host_len),
            service,
            clamp(service_len),
            map_bits(flags, NI_FLAGS, true),
        )
    };
    bionic_error(error)
}

/// Logs the first sockets and connections (diagnostics for the network
/// step); the calls themselves go to glibc unchanged (same arguments and
/// kernel structures in both).
fn first_few(counter: &std::sync::atomic::AtomicU32) -> bool {
    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 30
}

/// `int socket(domain, type, protocol)`, logged.
pub extern "C" fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int {
    // SAFETY: the engine's own call, passed through.
    let fd = unsafe { libc::socket(domain, kind, protocol) };
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if first_few(&COUNT) {
        eprintln!(
            "feline-runtime: socket(family {domain}, type {:#x}) -> {fd}",
            kind
        );
    }
    fd
}

/// `int connect(fd, address, length)`, logged with the destination.
///
/// # Safety
/// As C's.
pub unsafe extern "C" fn connect(
    fd: c_int,
    address: *const libc::sockaddr,
    length: libc::socklen_t,
) -> c_int {
    // SAFETY: the engine's own call, passed through.
    let result = unsafe { libc::connect(fd, address, length) };
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if first_few(&COUNT) {
        let errno = std::io::Error::last_os_error();
        // SAFETY: a socket address of `length` bytes, from the engine.
        let target = unsafe { describe(address, length) };
        let outcome = if result == 0 {
            "ok".to_owned()
        } else {
            errno.to_string()
        };
        eprintln!("feline-runtime: connect({fd}, {target}) -> {outcome}");
    }
    result
}

/// An IPv4/IPv6 address and port, as text.
///
/// # Safety
/// `address` is NULL or a socket address of `length` bytes.
unsafe fn describe(address: *const libc::sockaddr, length: libc::socklen_t) -> String {
    use std::net::{Ipv4Addr, Ipv6Addr};
    if address.is_null() {
        return "<none>".into();
    }
    // SAFETY: the family is the first field of every socket address.
    let family = c_int::from(unsafe { (*address).sa_family });
    let size = length as usize;
    match family {
        libc::AF_INET if size >= std::mem::size_of::<libc::sockaddr_in>() => {
            // SAFETY: an IPv4 address of the right size.
            let a = unsafe { &*address.cast::<libc::sockaddr_in>() };
            let ip = Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
            format!("{ip}:{}", u16::from_be(a.sin_port))
        }
        libc::AF_INET6 if size >= std::mem::size_of::<libc::sockaddr_in6>() => {
            // SAFETY: an IPv6 address of the right size.
            let a = unsafe { &*address.cast::<libc::sockaddr_in6>() };
            let ip = Ipv6Addr::from(a.sin6_addr.s6_addr);
            format!("[{ip}]:{}", u16::from_be(a.sin6_port))
        }
        libc::AF_UNIX => "a local (unix) socket".into(),
        other => format!("family {other}"),
    }
}

pub fn implementation(name: &str) -> Option<u64> {
    type GetAddrInfo = unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        *const BionicAddrinfo,
        *mut *mut BionicAddrinfo,
    ) -> c_int;
    type GetNameInfo = unsafe extern "C" fn(
        *const libc::sockaddr,
        libc::socklen_t,
        *mut c_char,
        usize,
        *mut c_char,
        usize,
        c_int,
    ) -> c_int;
    let address = match name {
        "getaddrinfo" => getaddrinfo as GetAddrInfo as usize,
        "freeaddrinfo" => freeaddrinfo as unsafe extern "C" fn(*mut BionicAddrinfo) as usize,
        "gai_strerror" => gai_strerror as extern "C" fn(c_int) -> *const c_char as usize,
        "getnameinfo" => getnameinfo as GetNameInfo as usize,
        "socket" => socket as extern "C" fn(c_int, c_int, c_int) -> c_int as usize,
        "connect" => {
            connect as unsafe extern "C" fn(c_int, *const libc::sockaddr, libc::socklen_t) -> c_int
                as usize
        }
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn layout_matches_bionic() {
        assert_eq!(std::mem::size_of::<BionicAddrinfo>(), 48);
        assert_eq!(std::mem::offset_of!(BionicAddrinfo, ai_canonname), 24);
        assert_eq!(std::mem::offset_of!(BionicAddrinfo, ai_addr), 32);
    }

    #[test]
    fn resolves_a_numeric_address_in_bionic_shape() {
        let hints = BionicAddrinfo {
            ai_flags: 0x4 | 0x8, // AI_NUMERICHOST | AI_NUMERICSERV (bionic)
            ai_family: libc::AF_INET,
            ai_socktype: libc::SOCK_STREAM,
            ai_protocol: 0,
            ai_addrlen: 0,
            ai_canonname: std::ptr::null_mut(),
            ai_addr: std::ptr::null_mut(),
            ai_next: std::ptr::null_mut(),
        };
        let mut res = std::ptr::null_mut();
        // SAFETY (test): valid strings, hints and output.
        let error =
            unsafe { getaddrinfo(c"127.0.0.1".as_ptr(), c"443".as_ptr(), &hints, &mut res) };
        assert_eq!(error, 0);
        // SAFETY (test): a list from getaddrinfo.
        unsafe {
            let first = &*res;
            assert_eq!(first.ai_family, libc::AF_INET);
            let addr = &*first.ai_addr.cast::<libc::sockaddr_in>();
            assert_eq!(u16::from_be(addr.sin_port), 443);
            assert_eq!(
                addr.sin_addr.s_addr,
                u32::from_be_bytes([127, 0, 0, 1]).to_be()
            );
            freeaddrinfo(res);
        }
        // A name with AI_NUMERICHOST fails with bionic's EAI_NONAME (8).
        // SAFETY (test): as above.
        let error =
            unsafe { getaddrinfo(c"not-a-number".as_ptr(), std::ptr::null(), &hints, &mut res) };
        assert_eq!(error, 8);
        // SAFETY (test): a static string.
        let text = unsafe { CStr::from_ptr(gai_strerror(8)) };
        assert!(!text.to_bytes().is_empty());
    }
}
