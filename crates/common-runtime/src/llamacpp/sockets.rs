//! Who owns a loopback port and who is connected to it (WP-04 C3; ADR-0005 D3 and D4). Before any
//! byte is forwarded, the bridge must know that the `127.0.0.1:<port>` listener belongs to the
//! supervised child and that every client of that port is one of the daemon's own sockets.
//!
//! The facts come from the OS, parsed by pure functions tested on fixtures:
//! - Linux: `/proc/net/tcp` and `/proc/net/tcp6` (proc(5)), with `/proc/<pid>/fd/*` links
//!   `socket:[inode]` saying which sockets a process holds. A socket's owner is its inode.
//! - macOS: `lsof -nP -iTCP:<port> -Fpn`. A socket's owner is its pid. Unprivileged `lsof` cannot
//!   see other users' sockets: a squatter run by another local user is ADR-0005 R11.
//!
//! The checks are written once over [`Sock`]; `owned` says whether an owner is ours (an inode of
//! the child or of the daemon on Linux, the pid on macOS). Only IPv4 loopback counts as owned; any
//! other listener on the port, at any address or family, is a squatter. Addresses are canonical:
//! a v4-mapped `::ffff:a.b.c.d` (an AF_INET6 socket talking IPv4) is read as `a.b.c.d`. A client is
//! told apart by its socket's owner and the endpoint it dials, never by its own source address:
//! another local user can connect to 127.0.0.1 from 127.0.0.2 (Linux) or the host's LAN address.
//!
//! Ported from the UCompute Core Lab (Apache-2.0); the macOS fixtures are real `lsof` output from
//! the lab's own `ggml-rpc-server` and client processes on this Mac (b11609, 2026-10-24).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// One TCP socket as the OS reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sock {
    /// Socket inode (Linux) or pid (macOS).
    pub owner: u64,
    pub local: SocketAddr,
    /// None for a listener.
    pub remote: Option<SocketAddr>,
    pub listening: bool,
}

const TCP_ESTABLISHED: u8 = 0x01;
const TCP_LISTEN: u8 = 0x0A;
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// `/proc/net/tcp` or `/proc/net/tcp6` -> sockets in LISTEN or ESTABLISHED state (owner = inode).
/// The header and malformed lines are skipped.
pub fn parse_proc_net_tcp(text: &str) -> Vec<Sock> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || !f[0].ends_with(':') {
                return None;
            }
            let state = u8::from_str_radix(f[3], 16).ok()?;
            if state != TCP_LISTEN && state != TCP_ESTABLISHED {
                return None;
            }
            let listening = state == TCP_LISTEN;
            Some(Sock {
                owner: f[9].parse().ok()?,
                local: proc_addr(f[1])?,
                remote: if listening { None } else { Some(proc_addr(f[2])?) },
                listening,
            })
        })
        .collect()
}

/// `0100007F:C9AB` -> 127.0.0.1:51627. The address is printed as 32-bit words in host (little-endian)
/// order; the port as big-endian hex.
fn proc_addr(s: &str) -> Option<SocketAddr> {
    let (addr, port) = s.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let words = |hex: &str| -> Option<Vec<u8>> {
        (0..hex.len() / 8)
            .map(|i| {
                u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16)
                    .ok()
                    .map(u32::to_le_bytes)
            })
            .collect::<Option<Vec<_>>>()
            .map(|w| w.concat())
    };
    let ip = match addr.len() {
        8 => {
            let b = words(addr)?;
            IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]))
        }
        32 => {
            let b: [u8; 16] = words(addr)?.try_into().ok()?;
            IpAddr::V6(Ipv6Addr::from(b))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip.to_canonical(), port))
}

/// A `/proc/<pid>/fd/*` link target `socket:[12345]` -> 12345.
pub fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
}

/// `lsof -nP … -Fpn` (or `-Fn` with `-p`): `p<pid>`, `f<fd>` and `n<name>` lines -> sockets (owner =
/// pid). Names: `127.0.0.1:P` (a listener), `127.0.0.1:A->127.0.0.1:B` (a connection), `*:P`, `[::1]:P`.
pub fn parse_lsof(text: &str) -> Vec<Sock> {
    let mut pid = None;
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(name), Some(owner)) = (line.strip_prefix('n'), pid) {
            let (local, remote) = match name.split_once("->") {
                Some((l, r)) => (lsof_addr(l), Some(lsof_addr(r))),
                None => (lsof_addr(name), None),
            };
            match (local, remote) {
                (Some(local), None) => out.push(Sock {
                    owner,
                    local,
                    remote: None,
                    listening: true,
                }),
                (Some(local), Some(Some(r))) => out.push(Sock {
                    owner,
                    local,
                    remote: Some(r),
                    listening: false,
                }),
                _ => {}
            }
        }
    }
    out
}

fn lsof_addr(s: &str) -> Option<SocketAddr> {
    let (host, port) = s.rsplit_once(':')?;
    let port = port.parse().ok()?;
    let ip = match host {
        "*" => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        h if h.starts_with('[') => IpAddr::V6(h.trim_start_matches('[').trim_end_matches(']').parse().ok()?),
        h => IpAddr::V4(h.parse().ok()?),
    };
    Some(SocketAddr::new(ip.to_canonical(), port))
}

/// What listens on a port: the three states a supervisor waiting for its child must tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listener {
    /// No listener yet.
    Nobody,
    /// Only ours, on 127.0.0.1.
    Ours,
    /// Something else listens on the port too: another owner, 0.0.0.0, `[::]` or `[::1]`.
    Squatted,
}

pub fn listener_state(socks: &[Sock], port: u16, owned: impl Fn(u64) -> bool) -> Listener {
    let mut ours = false;
    for s in socks.iter().filter(|s| s.listening && s.local.port() == port) {
        if !(owned(s.owner) && s.local.ip() == LOOPBACK) {
            return Listener::Squatted;
        }
        ours = true;
    }
    if ours {
        Listener::Ours
    } else {
        Listener::Nobody
    }
}

/// Every listener on `port`, at any address, is ours and on 127.0.0.1, and there is at least one.
/// Another listener on the port (another owner, 0.0.0.0, `[::]`) is a squatter: false.
pub fn listener_owned(socks: &[Sock], port: u16, owned: impl Fn(u64) -> bool) -> bool {
    listener_state(socks, port, owned) == Listener::Ours
}

/// The client ends of connections to 127.0.0.1:`port`, from any source address: (client port, owner).
pub fn established_clients(socks: &[Sock], port: u16) -> Vec<(u16, u64)> {
    let target = SocketAddr::new(LOOPBACK, port);
    socks
        .iter()
        .filter(|s| !s.listening && s.remote == Some(target))
        .map(|s| (s.local.port(), s.owner))
        .collect()
}

/// Clients of `port` that are not ours: any of them tears the session down (ADR-0005 D3).
pub fn foreign_clients(socks: &[Sock], port: u16, owned: impl Fn(u64) -> bool) -> Vec<(u16, u64)> {
    established_clients(socks, port)
        .into_iter()
        .filter(|&(_, o)| !owned(o))
        .collect()
}

/// The connection `client` -> 127.0.0.1:`local_port` exists and its client end is ours. `client` is
/// the whole accepted address: the same port from another source address is another socket.
pub fn conn_peer_owned(socks: &[Sock], local_port: u16, client: SocketAddr, owned: impl Fn(u64) -> bool) -> bool {
    let client = SocketAddr::new(client.ip().to_canonical(), client.port());
    socks.iter().any(|s| {
        !s.listening && s.local == client && s.remote == Some(SocketAddr::new(LOOPBACK, local_port)) && owned(s.owner)
    })
}

/// Thin readers over the live system (Linux).
#[cfg(target_os = "linux")]
pub mod live {
    use super::*;
    use std::collections::HashSet;
    use std::io;

    pub fn sockets() -> io::Result<Vec<Sock>> {
        let mut out = parse_proc_net_tcp(&std::fs::read_to_string("/proc/net/tcp")?);
        if let Ok(v6) = std::fs::read_to_string("/proc/net/tcp6") {
            out.extend(parse_proc_net_tcp(&v6));
        }
        Ok(out)
    }

    /// The socket inodes `pid` holds.
    pub fn inodes_of(pid: u32) -> io::Result<HashSet<u64>> {
        let mut out = HashSet::new();
        for entry in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
            if let Ok(target) = std::fs::read_link(entry?.path()) {
                if let Some(inode) = target.to_str().and_then(socket_inode) {
                    out.insert(inode);
                }
            }
        }
        Ok(out)
    }

    pub fn listener_owned_by(pid: u32, port: u16) -> io::Result<bool> {
        Ok(listener_state_of(pid, port)? == Listener::Ours)
    }

    pub fn listener_state_of(pid: u32, port: u16) -> io::Result<Listener> {
        let ours = inodes_of(pid)?;
        Ok(listener_state(&sockets()?, port, |i| ours.contains(&i)))
    }

    /// Clients of 127.0.0.1:`port` whose socket is not one of `pid`'s (the daemon's).
    pub fn foreign_clients_of(pid: u32, port: u16) -> io::Result<Vec<(u16, u64)>> {
        let ours = inodes_of(pid)?;
        Ok(foreign_clients(&sockets()?, port, |i| ours.contains(&i)))
    }

    /// (client port, inode) of every connection to 127.0.0.1:`port`.
    pub fn established_clients(port: u16) -> io::Result<Vec<(u16, u64)>> {
        Ok(super::established_clients(&sockets()?, port))
    }

    pub fn conn_peer_owned_by(pid: u32, local_port: u16, client: SocketAddr) -> io::Result<bool> {
        let ours = inodes_of(pid)?;
        Ok(conn_peer_owned(&sockets()?, local_port, client, |i| ours.contains(&i)))
    }
}

/// Thin readers over the live system (macOS, `lsof`: about 50–100 ms per call).
#[cfg(target_os = "macos")]
pub mod live {
    use super::*;
    use std::io;

    /// Every TCP socket of every visible process on `port`. lsof exits 1 when it finds nothing.
    pub fn sockets_on_port(port: u16) -> io::Result<Vec<Sock>> {
        let out = std::process::Command::new("/usr/sbin/lsof")
            .args(["-nP", &format!("-iTCP:{port}"), "-Fpn"])
            .output()?;
        Ok(parse_lsof(&String::from_utf8_lossy(&out.stdout)))
    }

    pub fn listener_owned_by(pid: u32, port: u16) -> io::Result<bool> {
        Ok(listener_state_of(pid, port)? == Listener::Ours)
    }

    pub fn listener_state_of(pid: u32, port: u16) -> io::Result<Listener> {
        Ok(listener_state(&sockets_on_port(port)?, port, |o| o == u64::from(pid)))
    }

    /// Clients of 127.0.0.1:`port` that `pid` (the daemon) does not hold.
    pub fn foreign_clients_of(pid: u32, port: u16) -> io::Result<Vec<(u16, u64)>> {
        Ok(foreign_clients(&sockets_on_port(port)?, port, |o| o == u64::from(pid)))
    }

    /// (client port, pid) of every connection to 127.0.0.1:`port`.
    pub fn established_clients(port: u16) -> io::Result<Vec<(u16, u64)>> {
        Ok(super::established_clients(&sockets_on_port(port)?, port))
    }

    pub fn conn_peer_owned_by(pid: u32, local_port: u16, client: SocketAddr) -> io::Result<bool> {
        Ok(conn_peer_owned(
            &sockets_on_port(local_port)?,
            local_port,
            client,
            |o| o == u64::from(pid),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real lsof output, this Mac, 2026-10-24: a ggml-rpc-server (pid 54988) listening on
    // 127.0.0.1:51627 with one accepted client (the "head", pid 54989); a foreign client (pid 54990)
    // queued in the backlog; a squatter (pid 54991) listening on 127.0.0.1:51631.
    const LSOF_PORT: &str = include_str!("fixtures/lsof-port.txt");
    const LSOF_P_RPC: &str = include_str!("fixtures/lsof-p-rpc.txt");
    const LSOF_SQUAT: &str = include_str!("fixtures/lsof-squat.txt");
    // proc(5)-format samples (the same ports): child inodes 1001-1002, daemon 2001, foreign 3001,
    // a 0.0.0.0:51631 listener 4001, the API port 5001, a malformed line; tcp6: a [::]:51627 squatter.
    const PROC_TCP: &str = include_str!("fixtures/proc-net-tcp.txt");
    const PROC_TCP6: &str = include_str!("fixtures/proc-net-tcp6.txt");
    const PORT: u16 = 51627;

    fn lo(port: u16) -> SocketAddr {
        SocketAddr::new(LOOPBACK, port)
    }

    #[test]
    fn proc_lines_decode_hex_addresses_and_ports() {
        let s = parse_proc_net_tcp(PROC_TCP);
        assert_eq!(s.len(), 6, "header and malformed line skipped: {s:?}");
        assert_eq!(s[0].local, "127.0.0.1:51627".parse().unwrap());
        assert!(s[0].listening && s[0].remote.is_none() && s[0].owner == 1001);
        assert_eq!(s[2].remote, Some("127.0.0.1:51627".parse().unwrap()));
        assert_eq!(s[4].local, "0.0.0.0:51631".parse().unwrap());
        let v6 = parse_proc_net_tcp(PROC_TCP6);
        assert_eq!(v6[0].local, "[::]:51627".parse().unwrap());
        assert_eq!(socket_inode("socket:[12345]"), Some(12345));
        assert_eq!(socket_inode("pipe:[12345]"), None);
        assert_eq!(socket_inode("anon_inode:[eventpoll]"), None);
    }

    #[test]
    fn linux_listener_clients_and_squatters() {
        let s = parse_proc_net_tcp(PROC_TCP);
        let child = |i: u64| i == 1001 || i == 1002;
        assert!(listener_owned(&s, PORT, child));
        assert!(
            !listener_owned(&s, PORT, |i| i == 2001),
            "the daemon does not own the child's listener"
        );
        assert_eq!(established_clients(&s, PORT), vec![(51632, 2001), (51633, 3001)]);
        assert_eq!(foreign_clients(&s, PORT, |i| i == 2001), vec![(51633, 3001)]);
        assert!(conn_peer_owned(&s, PORT, lo(51632), |i| i == 2001));
        assert!(!conn_peer_owned(&s, PORT, lo(51633), |i| i == 2001));
        // a listener on 0.0.0.0 is never owned, whoever holds it (IPv4 loopback only)
        assert!(!listener_owned(&s, 51631, |i| i == 4001));
        // an IPv6 squatter on the same port spoils it
        let mut both = s.clone();
        both.extend(parse_proc_net_tcp(PROC_TCP6));
        assert!(!listener_owned(&both, PORT, child));
        // no listener at all
        assert!(!listener_owned(&s, 40000, |_| true));
        // the three states a supervisor waits through
        assert_eq!(listener_state(&s, 40000, child), Listener::Nobody);
        assert_eq!(listener_state(&s, PORT, child), Listener::Ours);
        assert_eq!(listener_state(&both, PORT, child), Listener::Squatted);
        assert_eq!(listener_state(&s, 51631, |i| i == 4001), Listener::Squatted);
    }

    #[test]
    fn macos_real_lsof_listener_clients_and_squatters() {
        let s = parse_lsof(LSOF_PORT);
        assert!(listener_owned(&s, PORT, |o| o == 54988));
        assert!(!listener_owned(&s, PORT, |o| o == 54989));
        assert_eq!(established_clients(&s, PORT), vec![(51632, 54989), (51633, 54990)]);
        assert_eq!(foreign_clients(&s, PORT, |o| o == 54989), vec![(51633, 54990)]);
        assert!(conn_peer_owned(&s, PORT, lo(51632), |o| o == 54989));
        assert!(!conn_peer_owned(&s, PORT, lo(51633), |o| o == 54989));
        // `-p <pid> -Fn` output parses to the same sockets of that pid
        let p = parse_lsof(LSOF_P_RPC);
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|x| x.owner == 54988));
        // the squatter's port is held by pid 54991, not the expected child
        let q = parse_lsof(LSOF_SQUAT);
        assert!(!listener_owned(&q, 51631, |o| o == 54988));
        assert!(listener_owned(&q, 51631, |o| o == 54991));
        assert_eq!(listener_state(&q, 51631, |o| o == 54988), Listener::Squatted);
        assert_eq!(listener_state(&q, PORT, |o| o == 54988), Listener::Nobody);
    }

    // A real capture from sentinel (engineering, 2026-10-24 06:25 UTC, Linux 7.0.0-38-generic, b11609
    // Ubuntu CPU build): `ggml-rpc-server -H 127.0.0.1 -p 50113` (pid 301680), a squatter on
    // 127.0.0.1:50114 (pid 301730), a foreign client 42204 -> 50113 (pid 301731); LAN rows removed.
    const SENTINEL_TCP: &str = include_str!("fixtures/proc-net-tcp-sentinel.txt");
    const SENTINEL_FD: &str = include_str!("fixtures/proc-fd-sentinel.txt");

    fn inodes(pid: &str) -> Vec<u64> {
        SENTINEL_FD
            .lines()
            .filter_map(|l| {
                l.split_once(' ')
                    .filter(|(p, _)| *p == pid)
                    .and_then(|(_, link)| socket_inode(link))
            })
            .collect()
    }

    #[test]
    fn real_sentinel_capture_gives_the_expected_facts() {
        let s = parse_proc_net_tcp(SENTINEL_TCP);
        assert_eq!(s.len(), 7, "every real row parses, trailing spaces included: {s:?}");
        assert_eq!(s[0].local, "127.0.0.54:53".parse().unwrap()); // systemd-resolved's stub
        assert_eq!(s[4].local, "0.0.0.0:22".parse().unwrap()); // sshd
        let rpc = inodes("301680");
        assert_eq!(rpc, vec![991233, 991234], "the pipe link is not a socket");
        let ours = |i: u64| rpc.contains(&i);
        assert!(listener_owned(&s, 50113, ours), "listener_owned_by(301680, 50113)");
        assert!(!listener_owned(&s, 50114, ours), "50114 is squatted by 301730");
        assert!(listener_owned(&s, 50114, |i| inodes("301730").contains(&i)));
        assert_eq!(established_clients(&s, 50113), vec![(42204, 975856)]);
        assert!(
            !conn_peer_owned(&s, 50113, lo(42204), ours),
            "the client end 975856 is not one of 301680's fds"
        );
        assert!(conn_peer_owned(&s, 50113, lo(42204), |i| inodes("301731").contains(&i)));
        assert_eq!(foreign_clients(&s, 50113, ours), vec![(42204, 975856)]);
    }

    // proc(5)-format samples (synthetic): the daemon's client 2001 as in PROC_TCP, and three clients
    // of 127.0.0.1:51627 that do not come from 127.0.0.1: one bound to 127.0.0.2 (3001), one bound to
    // a LAN address 192.168.1.176 (3002), and an AF_INET6 socket dialling ::ffff:127.0.0.1 (3003).
    const OTHER_SOURCES_TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:C9AB 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1001 1 0000000000000000 100 0 0 10 0
   1: 0100007F:C9B0 0100007F:C9AB 01 00000000:00000000 00:00000000 00000000  1000        0 2001 1 0000000000000000 100 0 0 10 0
   2: 0200007F:C9B0 0100007F:C9AB 01 00000000:00000000 00:00000000 00000000  1001        0 3001 1 0000000000000000 100 0 0 10 0
   3: B001A8C0:C9B5 0100007F:C9AB 01 00000000:00000000 00:00000000 00000000  1001        0 3002 1 0000000000000000 100 0 0 10 0
";
    const OTHER_SOURCES_TCP6: &str = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0000000000000000FFFF00000100007F:C9B6 0000000000000000FFFF00000100007F:C9AB 01 00000000:00000000 00:00000000 00000000  1001        0 3003 1 0000000000000000 100 0 0 10 0
";

    /// A client is foreign whatever its source address or family; ownership of the engine's
    /// connection is the whole accepted address, not its port (review NX-1, NX-2).
    #[test]
    fn clients_from_other_local_addresses_or_v4_mapped_are_foreign() {
        let mut s = parse_proc_net_tcp(OTHER_SOURCES_TCP);
        s.extend(parse_proc_net_tcp(OTHER_SOURCES_TCP6));
        assert_eq!(s[4].local, "127.0.0.1:51638".parse().unwrap(), "v4-mapped read as IPv4");
        assert_eq!(s[4].remote, Some(lo(PORT)));
        let daemon = |i: u64| i == 2001;
        assert_eq!(
            foreign_clients(&s, PORT, daemon),
            vec![(51632, 3001), (51637, 3002), (51638, 3003)]
        );
        // 127.0.0.2:51632 reuses the daemon's client port: it is not the daemon's connection
        assert!(conn_peer_owned(&s, PORT, lo(51632), daemon));
        assert!(!conn_peer_owned(&s, PORT, "127.0.0.2:51632".parse().unwrap(), daemon));
        assert!(conn_peer_owned(
            &s,
            PORT,
            "[::ffff:127.0.0.1]:51632".parse().unwrap(),
            daemon
        ));
        // the same on macOS: a LAN-bound client of 127.0.0.1 in the form lsof printed on this Mac
        // (2026-10-29), next to the engine's own connection with the same client port, and a
        // synthetic bracketed v4-mapped name
        let m = parse_lsof(
            "p10\nf3\nn127.0.0.1:51049\np11\nf3\nn127.0.0.1:51051->127.0.0.1:51049\n\
             p12\nf3\nn192.168.1.176:51051->127.0.0.1:51049\np13\nf3\nn[::ffff:127.0.0.1]:51052->[::ffff:127.0.0.1]:51049\n",
        );
        assert_eq!(foreign_clients(&m, 51049, |o| o == 11), vec![(51051, 12), (51052, 13)]);
        assert!(conn_peer_owned(&m, 51049, lo(51051), |o| o == 11));
        assert!(!conn_peer_owned(
            &m,
            51049,
            "192.168.1.176:51051".parse().unwrap(),
            |o| o == 11
        ));
    }

    #[test]
    fn lsof_names_other_families_and_garbage() {
        let s = parse_lsof(
            "p7\nf3\nn*:51627\nf4\nn[::1]:51627\nf5\nnnot an address\nx-unknown field\nf6\nn127.0.0.1:1->\n",
        );
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s[0].local, "0.0.0.0:51627".parse().unwrap());
        assert_eq!(s[1].local, "[::1]:51627".parse().unwrap());
        // both are listeners on the port at non-IPv4-loopback addresses: never owned
        assert!(!listener_owned(&s, PORT, |_| true));
        assert!(
            parse_lsof("n127.0.0.1:1\n").is_empty(),
            "a name before any pid line is skipped"
        );
    }
}
