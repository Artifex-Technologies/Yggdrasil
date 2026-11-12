//! The kernel's last scan results, read over generic netlink (nl80211): NL80211_CMD_GET_SCAN as a dump. Any user may ask for
//! it (no root, no capability); it reads what the kernel already has, sends nothing on the radio and starts no scan. The only
//! commands this file can send are CTRL_CMD_GETFAMILY (the nl80211 family's number) and NL80211_CMD_GET_SCAN.
//!
//! The socket calls are the only unsafe code in the crate; the messages are read by the safe, bounds-checked functions below
//! (`messages`, `attrs`, `parse_bss`), which are tested with random input.

use crate::Bss;
use std::io;

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x01;
const NLM_F_DUMP: u16 = 0x300;
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const NL80211_CMD_GET_SCAN: u8 = 32;
const NL80211_ATTR_IFINDEX: u16 = 3;
const NL80211_ATTR_BSS: u16 = 47;
const BSS_BSSID: u16 = 1;
const BSS_FREQUENCY: u16 = 2;
const BSS_BEACON_INTERVAL: u16 = 4;
const BSS_CAPABILITY: u16 = 5;
const BSS_INFORMATION_ELEMENTS: u16 = 6;
const BSS_SIGNAL_MBM: u16 = 7;
const BSS_STATUS: u16 = 9;
const BSS_SEEN_MS_AGO: u16 = 10;
const BSS_BEACON_IES: u16 = 11;
const BSS_STATUS_ASSOCIATED: u32 = 1;
/// The most element bytes kept from one access point (a frame cannot carry much more than 2,300).
const MAX_IES: usize = 4096;
/// A dump message is at most 32 KiB (the kernel's own limit); twice that leaves room.
const BUF: usize = 64 * 1024;
/// The most receives one dump may take.
const MAX_READS: usize = 400;

/// The access points the kernel last saw on interface `ifname` (wlan0, wlp3s0 ...).
pub fn scan_dump(ifname: &str) -> io::Result<Vec<Bss>> {
    let ifindex = if_index(ifname)?;
    let s = Sock::open()?;
    let family = s.family("nl80211")?;
    s.send(&request(family, NLM_F_REQUEST | NLM_F_DUMP, 2, NL80211_CMD_GET_SCAN, &[(NL80211_ATTR_IFINDEX, &ifindex.to_ne_bytes())]))?;
    let mut out = Vec::new();
    let mut buf = vec![0u8; BUF];
    for _ in 0..MAX_READS {
        let n = s.recv(&mut buf)?;
        for (kind, _flags, seq, payload) in messages(&buf[..n]) {
            if seq != 2 {
                continue;
            }
            match kind {
                NLMSG_DONE => return Ok(out),
                NLMSG_ERROR => {
                    let code = payload.get(..4).map(|b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(-libc::EIO);
                    if code < 0 {
                        return Err(io::Error::from_raw_os_error(-code));
                    }
                }
                k if k == family => {
                    if let Some(b) =
                        payload.get(4..).and_then(|a| attrs(a).find(|(t, _)| *t == NL80211_ATTR_BSS)).and_then(|(_, v)| parse_bss(v))
                    {
                        if out.len() < crate::MAX_BSS {
                            out.push(b);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "the scan list did not end"))
}

/// The interface's number.
fn if_index(ifname: &str) -> io::Result<u32> {
    if ifname.is_empty() || ifname.len() > 15 || ifname.contains(['/', '\0']) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an interface name"));
    }
    let n = std::fs::read_to_string(format!("/sys/class/net/{ifname}/ifindex"))?;
    n.trim().parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "no interface number"))
}

/// A netlink request: header, generic netlink header (command, version 1), attributes. Native byte order, as netlink uses.
fn request(kind: u16, flags: u16, seq: u32, cmd: u8, attrs: &[(u16, &[u8])]) -> Vec<u8> {
    let mut m = vec![0u8; 16];
    m.extend_from_slice(&[cmd, 1, 0, 0]);
    for (t, v) in attrs {
        m.extend_from_slice(&((4 + v.len()) as u16).to_ne_bytes());
        m.extend_from_slice(&t.to_ne_bytes());
        m.extend_from_slice(v);
        while !m.len().is_multiple_of(4) {
            m.push(0);
        }
    }
    let len = m.len() as u32;
    m[0..4].copy_from_slice(&len.to_ne_bytes());
    m[4..6].copy_from_slice(&kind.to_ne_bytes());
    m[6..8].copy_from_slice(&flags.to_ne_bytes());
    m[8..12].copy_from_slice(&seq.to_ne_bytes());
    m
}

fn align4(n: usize) -> usize {
    n.saturating_add(3) & !3
}

/// The netlink messages in one receive: (type, flags, sequence number, payload). Stops at a header that does not fit.
pub fn messages(buf: &[u8]) -> impl Iterator<Item = (u16, u16, u32, &[u8])> {
    let mut rest = buf;
    std::iter::from_fn(move || {
        let h = rest.get(..16)?;
        let len = u32::from_ne_bytes([h[0], h[1], h[2], h[3]]) as usize;
        if len < 16 || len > rest.len() {
            rest = &[];
            return None;
        }
        let kind = u16::from_ne_bytes([h[4], h[5]]);
        let flags = u16::from_ne_bytes([h[6], h[7]]);
        let seq = u32::from_ne_bytes([h[8], h[9], h[10], h[11]]);
        let payload = &rest[16..len];
        rest = rest.get(align4(len)..).unwrap_or(&[]);
        Some((kind, flags, seq, payload))
    })
}

/// Netlink attributes: (type without the nested and byte-order flags, payload). Stops at one that does not fit.
pub fn attrs(buf: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut rest = buf;
    std::iter::from_fn(move || {
        let h = rest.get(..4)?;
        let len = u16::from_ne_bytes([h[0], h[1]]) as usize;
        if len < 4 || len > rest.len() {
            rest = &[];
            return None;
        }
        let kind = u16::from_ne_bytes([h[2], h[3]]) & 0x3fff;
        let payload = &rest[4..len];
        rest = rest.get(align4(len)..).unwrap_or(&[]);
        Some((kind, payload))
    })
}

fn u32_of(v: &[u8]) -> Option<u32> {
    v.get(..4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
}

/// One NL80211_ATTR_BSS (its nested attributes). None without an address.
pub fn parse_bss(nested: &[u8]) -> Option<Bss> {
    let mut b = Bss::default();
    let mut have = false;
    for (t, v) in attrs(nested) {
        match t {
            BSS_BSSID if v.len() >= 6 => {
                b.bssid.copy_from_slice(&v[..6]);
                have = true;
            }
            BSS_FREQUENCY => b.freq_mhz = u32_of(v).unwrap_or(0),
            BSS_BEACON_INTERVAL if v.len() >= 2 => b.beacon_interval = u16::from_ne_bytes([v[0], v[1]]),
            BSS_CAPABILITY if v.len() >= 2 => b.capability = u16::from_ne_bytes([v[0], v[1]]),
            BSS_INFORMATION_ELEMENTS => b.ies = v[..v.len().min(MAX_IES)].to_vec(),
            BSS_SIGNAL_MBM => b.signal_mbm = u32_of(v).map(|x| x as i32),
            BSS_STATUS => b.associated = u32_of(v) == Some(BSS_STATUS_ASSOCIATED),
            BSS_SEEN_MS_AGO => b.seen_ms_ago = u32_of(v),
            BSS_BEACON_IES => b.beacon_ies = v[..v.len().min(MAX_IES)].to_vec(),
            _ => {}
        }
    }
    have.then_some(b)
}

/// A generic netlink socket, closed when dropped.
struct Sock(libc::c_int);

impl Drop for Sock {
    fn drop(&mut self) {
        // SAFETY: the descriptor is ours and closed once.
        unsafe { libc::close(self.0) };
    }
}

impl Sock {
    fn open() -> io::Result<Sock> {
        // SAFETY: plain system calls on values made here; every pointer is to a live local of the size given.
        unsafe {
            let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_GENERIC);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let s = Sock(fd);
            let mut addr: libc::sockaddr_nl = std::mem::zeroed();
            addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
            if libc::bind(
                fd,
                &addr as *const libc::sockaddr_nl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            // never wait more than two seconds for the kernel
            let tv = libc::timeval { tv_sec: 2, tv_usec: 0 };
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const libc::timeval as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
            Ok(s)
        }
    }

    fn send(&self, msg: &[u8]) -> io::Result<()> {
        // SAFETY: the address is a zeroed sockaddr_nl (the kernel: port 0); msg is a live slice of its length.
        let n = unsafe {
            let mut to: libc::sockaddr_nl = std::mem::zeroed();
            to.nl_family = libc::AF_NETLINK as libc::sa_family_t;
            libc::sendto(
                self.0,
                msg.as_ptr() as *const libc::c_void,
                msg.len(),
                0,
                &to as *const libc::sockaddr_nl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// One datagram. MSG_TRUNC makes a message longer than the buffer an error rather than a silent cut.
    fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            // SAFETY: buf is a live, writable slice of its length.
            let n = unsafe { libc::recv(self.0, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), libc::MSG_TRUNC) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            let n = n as usize;
            if n > buf.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a netlink message was larger than expected"));
            }
            return Ok(n);
        }
    }

    /// The number of a generic netlink family ("nl80211").
    fn family(&self, name: &str) -> io::Result<u16> {
        let mut n = name.as_bytes().to_vec();
        n.push(0);
        self.send(&request(GENL_ID_CTRL, NLM_F_REQUEST, 1, CTRL_CMD_GETFAMILY, &[(CTRL_ATTR_FAMILY_NAME, &n)]))?;
        let mut buf = vec![0u8; BUF];
        for _ in 0..8 {
            let got = self.recv(&mut buf)?;
            for (kind, _, seq, payload) in messages(&buf[..got]) {
                if seq != 1 {
                    continue;
                }
                if kind == NLMSG_ERROR {
                    let code = payload.get(..4).map(|b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(-libc::EIO);
                    return Err(io::Error::from_raw_os_error(if code < 0 { -code } else { libc::ENOENT }));
                }
                if kind == GENL_ID_CTRL {
                    if let Some((_, v)) = payload.get(4..).and_then(|a| attrs(a).find(|(t, v)| *t == CTRL_ATTR_FAMILY_ID && v.len() >= 2)) {
                        return Ok(u16::from_ne_bytes([v[0], v[1]]));
                    }
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "no nl80211 (no Wi-Fi driver loaded?)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(t: u16, v: &[u8]) -> Vec<u8> {
        let mut a = ((4 + v.len()) as u16).to_ne_bytes().to_vec();
        a.extend_from_slice(&t.to_ne_bytes());
        a.extend_from_slice(v);
        while !a.len().is_multiple_of(4) {
            a.push(0);
        }
        a
    }

    #[test]
    fn a_dump_message_is_read() {
        let mut bss = attr(BSS_BSSID, &[2, 0, 0, 0, 0, 1]);
        bss.extend(attr(BSS_FREQUENCY, &5180u32.to_ne_bytes()));
        bss.extend(attr(BSS_SIGNAL_MBM, &(-5200i32).to_ne_bytes()));
        bss.extend(attr(BSS_STATUS, &1u32.to_ne_bytes()));
        bss.extend(attr(BSS_SEEN_MS_AGO, &900u32.to_ne_bytes()));
        bss.extend(attr(BSS_CAPABILITY, &0x0411u16.to_ne_bytes()));
        bss.extend(attr(BSS_INFORMATION_ELEMENTS, &[0, 4, b'T', b'e', b's', b't']));
        let body = attr(NL80211_ATTR_BSS | 0x8000, &bss);
        let msg = request(30, 2, 2, 34, &[]);
        let mut msg = msg[..20].to_vec();
        msg.extend(body);
        let len = msg.len() as u32;
        msg[0..4].copy_from_slice(&len.to_ne_bytes());
        let mut two = msg.clone();
        two.extend(request(NLMSG_DONE, 2, 2, 0, &[]));
        let got: Vec<_> = messages(&two).collect();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].0, got[1].0), (30, NLMSG_DONE));
        let (_, nested) = attrs(&got[0].3[4..]).next().unwrap();
        let b = parse_bss(nested).unwrap();
        assert_eq!(b.bssid, [2, 0, 0, 0, 0, 1]);
        assert_eq!((b.freq_mhz, b.signal_mbm, b.associated, b.seen_ms_ago, b.capability), (5180, Some(-5200), true, Some(900), 0x0411));
        assert_eq!(b.ies, vec![0, 4, b'T', b'e', b's', b't']);
        // no address, no access point; a length past the end stops the walk
        assert_eq!(parse_bss(&attr(BSS_FREQUENCY, &[1, 2, 3, 4])), None);
        assert_eq!(attrs(&[200, 0, 1, 0, 9]).count(), 0);
        assert_eq!(messages(&[255, 255, 0, 0]).count(), 0);
    }

    #[test]
    fn the_requests_are_only_these_two() {
        let r = request(GENL_ID_CTRL, NLM_F_REQUEST, 1, CTRL_CMD_GETFAMILY, &[(CTRL_ATTR_FAMILY_NAME, b"nl80211\0")]);
        assert_eq!(r.len() % 4, 0);
        assert_eq!(u32::from_ne_bytes([r[0], r[1], r[2], r[3]]) as usize, r.len());
        assert_eq!(r[16], CTRL_CMD_GETFAMILY);
        let s = request(28, NLM_F_REQUEST | NLM_F_DUMP, 2, NL80211_CMD_GET_SCAN, &[(NL80211_ATTR_IFINDEX, &3u32.to_ne_bytes())]);
        assert_eq!(s[16], 32, "GET_SCAN reads the list; TRIGGER_SCAN (33) is never sent");
        assert!(if_index("../etc").is_err() && if_index("").is_err());
    }
}
