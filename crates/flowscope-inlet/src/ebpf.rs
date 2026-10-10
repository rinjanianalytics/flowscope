// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! eBPF program spreading packets round-robin over the SO_REUSEPORT
//! sockets. Port of `inlet/flow/input/udp/reuseport_linux.go`.
//!
//! The Go version loads `reuseport_kern.c` compiled with clang. The program
//! is small, so it is assembled here and loaded with the bpf() syscall,
//! without an eBPF library:
//!
//! ```c
//! __u32 key = 0;
//! __u32 *counter = bpf_map_lookup_elem(&counter_map, &key);
//! if (!counter) return SK_PASS;
//! __u32 index = (*counter)++ % num_sockets;
//! bpf_sk_select_reuseport(reuse_md, &socket_map, &index, 0);
//! return SK_PASS;
//! ```
//!
//! It needs CAP_BPF (or root) and is optional: without it, the kernel spreads
//! packets using a hash of the source.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const BPF_MAP_CREATE: libc::c_long = 0;
const BPF_MAP_UPDATE_ELEM: libc::c_long = 2;
const BPF_PROG_LOAD: libc::c_long = 5;

const BPF_MAP_TYPE_PERCPU_ARRAY: u32 = 6;
const BPF_MAP_TYPE_REUSEPORT_SOCKARRAY: u32 = 20;
const BPF_PROG_TYPE_SK_REUSEPORT: u32 = 21;
const SO_ATTACH_REUSEPORT_EBPF: libc::c_int = 52;

const HELPER_MAP_LOOKUP_ELEM: i32 = 1;
const HELPER_SK_SELECT_REUSEPORT: i32 = 82;
const SK_PASS: i32 = 1;
const MAX_SOCKETS: u32 = 256;

/// One eBPF instruction.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Insn {
    code: u8,
    regs: u8,
    off: i16,
    imm: i32,
}

const fn insn(code: u8, dst: u8, src: u8, off: i16, imm: i32) -> Insn {
    Insn { code, regs: (src << 4) | (dst & 0x0f), off, imm }
}

/// Load a map file descriptor (two instructions).
const fn ld_map_fd(dst: u8, fd: i32) -> [Insn; 2] {
    // BPF_LD | BPF_DW | BPF_IMM with src = BPF_PSEUDO_MAP_FD.
    [insn(0x18, dst, 1, 0, fd), insn(0, 0, 0, 0, 0)]
}

/// Assemble the program.
fn program(counter_map: i32, socket_map: i32, num_sockets: u32) -> Vec<Insn> {
    let mut p = vec![
        insn(0xbf, 6, 1, 0, 0),   // r6 = r1 (context)
        insn(0x62, 10, 0, -4, 0), // *(u32 *)(r10 - 4) = 0 (key)
        insn(0xbf, 2, 10, 0, 0),  // r2 = r10
        insn(0x07, 2, 0, 0, -4),  // r2 += -4
    ];
    p.extend(ld_map_fd(1, counter_map)); // r1 = counter_map
    p.extend([
        insn(0x85, 0, 0, 0, HELPER_MAP_LOOKUP_ELEM), // call bpf_map_lookup_elem
        insn(0x15, 0, 0, 13, 0),                     // if r0 == 0 goto pass
        insn(0x61, 1, 0, 0, 0),                      // w1 = *(u32 *)(r0 + 0)
        insn(0xbc, 2, 1, 0, 0),                      // w2 = w1
        insn(0x04, 2, 0, 0, 1),                      // w2 += 1
        insn(0x63, 0, 2, 0, 0),                      // *(u32 *)(r0 + 0) = w2
        insn(0x94, 1, 0, 0, num_sockets as i32),     // w1 %= num_sockets
        insn(0x63, 10, 1, -8, 0),                    // *(u32 *)(r10 - 8) = w1 (index)
        insn(0xbf, 1, 6, 0, 0),                      // r1 = r6
    ]);
    p.extend(ld_map_fd(2, socket_map)); // r2 = socket_map
    p.extend([
        insn(0xbf, 3, 10, 0, 0),                         // r3 = r10
        insn(0x07, 3, 0, 0, -8),                         // r3 += -8
        insn(0xb7, 4, 0, 0, 0),                          // r4 = 0
        insn(0x85, 0, 0, 0, HELPER_SK_SELECT_REUSEPORT), // call bpf_sk_select_reuseport
        insn(0xb7, 0, 0, 0, SK_PASS),                    // pass: r0 = SK_PASS
        insn(0x95, 0, 0, 0, 0),                          // exit
    ]);
    p
}

fn bpf(cmd: libc::c_long, attr: &mut [u8; 128]) -> io::Result<i64> {
    // SAFETY: attr is a zero-initialized bpf_attr of the maximum size we use.
    let rc = unsafe { libc::syscall(libc::SYS_bpf, cmd, attr.as_mut_ptr(), attr.len() as u32) };
    if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(rc) }
}

fn put_u32(attr: &mut [u8; 128], offset: usize, v: u32) {
    attr[offset..offset + 4].copy_from_slice(&v.to_ne_bytes());
}

fn put_u64(attr: &mut [u8; 128], offset: usize, v: u64) {
    attr[offset..offset + 8].copy_from_slice(&v.to_ne_bytes());
}

fn map_create(map_type: u32, key_size: u32, value_size: u32, max_entries: u32) -> io::Result<OwnedFd> {
    let mut attr = [0u8; 128];
    put_u32(&mut attr, 0, map_type);
    put_u32(&mut attr, 4, key_size);
    put_u32(&mut attr, 8, value_size);
    put_u32(&mut attr, 12, max_entries);
    let fd = bpf(BPF_MAP_CREATE, &mut attr)?;
    // SAFETY: the kernel returned a new file descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

fn map_update(map: &OwnedFd, key: u32, value: u64) -> io::Result<()> {
    let mut attr = [0u8; 128];
    put_u32(&mut attr, 0, map.as_raw_fd() as u32);
    put_u64(&mut attr, 8, &key as *const u32 as u64);
    put_u64(&mut attr, 16, &value as *const u64 as u64);
    bpf(BPF_MAP_UPDATE_ELEM, &mut attr).map(|_| ())
}

fn prog_load(insns: &[Insn]) -> io::Result<OwnedFd> {
    let license = c"GPL";
    let mut log = vec![0u8; 4096];
    let mut attr = [0u8; 128];
    put_u32(&mut attr, 0, BPF_PROG_TYPE_SK_REUSEPORT);
    put_u32(&mut attr, 4, insns.len() as u32);
    put_u64(&mut attr, 8, insns.as_ptr() as u64);
    put_u64(&mut attr, 16, license.as_ptr() as u64);
    put_u32(&mut attr, 24, 1); // log level
    put_u32(&mut attr, 28, log.len() as u32);
    put_u64(&mut attr, 32, log.as_mut_ptr() as u64);
    match bpf(BPF_PROG_LOAD, &mut attr) {
        // SAFETY: the kernel returned a new file descriptor.
        Ok(fd) => Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) }),
        Err(e) => {
            let end = log.iter().position(|&b| b == 0).unwrap_or(log.len());
            let msg = String::from_utf8_lossy(&log[..end]).trim().to_string();
            if e.raw_os_error() == Some(libc::EPERM) {
                Err(io::Error::new(e.kind(), "operation not permitted (BPF capability missing or MEMLOCK too low)"))
            } else if msg.is_empty() {
                Err(e)
            } else {
                Err(io::Error::new(e.kind(), format!("{e}: {msg}")))
            }
        }
    }
}

/// The program attached to a group of sockets. Dropping it releases the
/// kernel objects (the program stays attached to the sockets until they are
/// closed).
pub struct ReuseportProgram {
    _prog: OwnedFd,
    _counter: OwnedFd,
    _sockets: OwnedFd,
}

impl ReuseportProgram {
    /// Load the program and attach it to the sockets.
    pub fn attach(fds: &[i32]) -> io::Result<ReuseportProgram> {
        if fds.is_empty() || fds.len() > MAX_SOCKETS as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid number of sockets"));
        }
        let counter = map_create(BPF_MAP_TYPE_PERCPU_ARRAY, 4, 8, 1)?;
        let sockets = map_create(BPF_MAP_TYPE_REUSEPORT_SOCKARRAY, 4, 8, MAX_SOCKETS)?;
        let insns = program(counter.as_raw_fd(), sockets.as_raw_fd(), fds.len() as u32);
        let prog = prog_load(&insns)?;
        for (i, fd) in fds.iter().enumerate() {
            map_update(&sockets, i as u32, *fd as u64)
                .map_err(|e| io::Error::new(e.kind(), format!("failed to update eBPF map: {e}")))?;
        }
        let prog_fd: libc::c_int = prog.as_raw_fd();
        // SAFETY: valid socket and option value.
        let rc = unsafe {
            libc::setsockopt(
                fds[0],
                libc::SOL_SOCKET,
                SO_ATTACH_REUSEPORT_EBPF,
                &prog_fd as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            let e = io::Error::last_os_error();
            return Err(io::Error::new(e.kind(), format!("failed to attach eBPF program: {e}")));
        }
        Ok(ReuseportProgram { _prog: prog, _counter: counter, _sockets: sockets })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble() {
        let p = program(3, 4, 2);
        assert_eq!(p.len(), 23);
        // The jump lands on "r0 = SK_PASS".
        let jump = p.iter().position(|i| i.code == 0x15).unwrap();
        assert_eq!(p[jump + 1 + p[jump].off as usize], insn(0xb7, 0, 0, 0, SK_PASS));
        assert_eq!(p[4], insn(0x18, 1, 1, 0, 3));
        assert_eq!(p[4].regs, 0x11);
        assert_eq!(p[2].regs, 0xa2);
    }

    #[test]
    fn attach_when_permitted() {
        // Needs CAP_BPF. Without it, check we get a clean error.
        let sockets: Vec<std::net::UdpSocket> = (0..2)
            .map(|_| {
                let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
                s.set_reuse_port(true).unwrap();
                s.bind(&"127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap().into()).unwrap();
                s.into()
            })
            .collect();
        let fds: Vec<i32> = sockets.iter().map(|s| s.as_raw_fd()).collect();
        match ReuseportProgram::attach(&fds) {
            Ok(_) => {}
            Err(e) => assert!(
                e.raw_os_error() == Some(libc::EPERM) || e.to_string().contains("not permitted"),
                "unexpected error: {e}"
            ),
        }
    }
}
