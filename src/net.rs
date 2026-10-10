//! UDP multicast transport (UDP模拟广播) and a token-bucket rate limiter.
//!
//! The "broadcast" is real IPv4 multicast on a local interface (loopback by default): one sender,
//! any number of receivers sharing the port via `SO_REUSEADDR`/`SO_REUSEPORT`.

use crate::receiver::{DatagramSource, Recv};
use crate::sender::PacketSink;
use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 1);
pub const DEFAULT_PORT: u16 = 47000;
pub const DEFAULT_IFACE: Ipv4Addr = Ipv4Addr::LOCALHOST;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MulticastConfig {
    pub group: Ipv4Addr,
    pub port: u16,
    /// Local interface address used to send/join (127.0.0.1 = loopback simulation).
    pub iface: Ipv4Addr,
    pub ttl: u32,
}

impl Default for MulticastConfig {
    fn default() -> Self {
        Self {
            group: DEFAULT_GROUP,
            port: DEFAULT_PORT,
            iface: DEFAULT_IFACE,
            ttl: 1,
        }
    }
}

/// Token bucket limiting the send rate in bytes per second.
pub struct RateLimiter {
    bytes_per_sec: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    /// `kbps == 0` disables limiting.
    pub fn new(kbps: u64) -> Self {
        let bytes_per_sec = kbps as f64 * 1000.0 / 8.0;
        let burst = (bytes_per_sec * 0.005).max(16.0 * 1024.0);
        Self {
            bytes_per_sec,
            burst,
            tokens: burst,
            last: Instant::now(),
        }
    }

    pub fn acquire(&mut self, bytes: usize) {
        if self.bytes_per_sec <= 0.0 {
            return;
        }
        loop {
            let now = Instant::now();
            self.tokens =
                (self.tokens + now.duration_since(self.last).as_secs_f64() * self.bytes_per_sec).min(self.burst);
            self.last = now;
            if self.tokens >= bytes as f64 {
                self.tokens -= bytes as f64;
                return;
            }
            let wait = (bytes as f64 - self.tokens) / self.bytes_per_sec;
            thread::sleep(Duration::from_secs_f64(wait.max(0.000_2)));
        }
    }
}

/// Multicast sender socket with rate limiting.
pub struct MulticastSender {
    socket: UdpSocket,
    dest: SocketAddr,
    limiter: RateLimiter,
    pub datagrams: u64,
    pub bytes: u64,
}

impl MulticastSender {
    pub fn new(cfg: &MulticastConfig, rate_kbps: u64) -> io::Result<Self> {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_multicast_if_v4(&cfg.iface)?;
        s.set_multicast_loop_v4(true)?;
        s.set_multicast_ttl_v4(cfg.ttl)?;
        let _ = s.set_send_buffer_size(4 << 20);
        s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into())?;
        Ok(Self {
            socket: s.into(),
            dest: SocketAddrV4::new(cfg.group, cfg.port).into(),
            limiter: RateLimiter::new(rate_kbps),
            datagrams: 0,
            bytes: 0,
        })
    }
}

impl PacketSink for MulticastSender {
    fn send(&mut self, datagram: &[u8]) -> io::Result<()> {
        self.limiter.acquire(datagram.len());
        loop {
            match self.socket.send_to(datagram, self.dest) {
                Ok(_) => break,
                // The kernel send buffer is momentarily full (ENOBUFS): back off and retry.
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || is_no_buffer_space(&e) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
        self.datagrams += 1;
        self.bytes += datagram.len() as u64;
        Ok(())
    }
}

fn is_no_buffer_space(e: &io::Error) -> bool {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const ENOBUFS: i32 = 55;
    #[cfg(target_os = "linux")]
    const ENOBUFS: i32 = 105;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "linux")))]
    const ENOBUFS: i32 = i32::MIN;
    e.raw_os_error() == Some(ENOBUFS)
}

/// Multicast receiver socket implementing [`DatagramSource`].
pub struct MulticastReceiver {
    socket: UdpSocket,
    buf: Vec<u8>,
}

impl MulticastReceiver {
    pub fn new(cfg: &MulticastConfig) -> io::Result<Self> {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_reuse_address(true)?;
        s.set_reuse_port(true)?;
        // Large buffer so bursts are not dropped by the kernel; the OS may clamp it.
        let _ = s.set_recv_buffer_size(4 << 20);
        s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, cfg.port).into())?;
        s.join_multicast_v4(&cfg.group, &cfg.iface)?;
        let socket: UdpSocket = s.into();
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self {
            socket,
            buf: vec![0u8; crate::wire::MAX_DATAGRAM],
        })
    }

    pub fn recv_buffer_size(&self) -> io::Result<usize> {
        socket2::SockRef::from(&self.socket).recv_buffer_size()
    }
}

impl DatagramSource for MulticastReceiver {
    fn recv(&mut self) -> io::Result<Recv> {
        match self.socket.recv(&mut self.buf) {
            Ok(n) => Ok(Recv::Datagram(self.buf[..n].to_vec())),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(Recv::Timeout),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(Recv::Timeout),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_paces() {
        // 8 Mbit/s = 1 MB/s: 200 KB beyond the initial burst should take roughly 0.2 s.
        let mut rl = RateLimiter::new(8_000);
        let start = Instant::now();
        for _ in 0..200 {
            rl.acquire(1000);
        }
        let elapsed = start.elapsed().as_secs_f64();
        assert!(elapsed > 0.15 && elapsed < 0.6, "elapsed {elapsed}");
        let mut unlimited = RateLimiter::new(0);
        let start = Instant::now();
        for _ in 0..10_000 {
            unlimited.acquire(1000);
        }
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
