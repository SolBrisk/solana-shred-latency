//! Linux SO_TIMESTAMPNS observes software ingress rather than receiver-thread
//! wakeup. Both sources share one fixed realtime-to-Instant mapping. Wall-clock
//! steps >5ms invalidate the run instead of silently changing its time basis.
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Clock {
    anchor: Instant,
    unix_ns: i128,
}
impl Default for Clock {
    fn default() -> Self {
        let before = Instant::now();
        let unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i128;
        let after = Instant::now();
        Self {
            anchor: before + after.duration_since(before) / 2,
            unix_ns,
        }
    }
}
impl Clock {
    fn stamp(&self, unix_ns: i128) -> Option<Instant> {
        let delta = unix_ns - self.unix_ns;
        let duration = std::time::Duration::from_nanos(u64::try_from(delta.unsigned_abs()).ok()?);
        if delta >= 0 {
            self.anchor.checked_add(duration)
        } else {
            self.anchor.checked_sub(duration)
        }
    }
    pub fn stepped(&self) -> bool {
        let now = Instant::now();
        let unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i128;
        (unix_ns - self.unix_ns - now.duration_since(self.anchor).as_nanos() as i128).unsigned_abs()
            > 5_000_000
    }
}
pub struct Datagram {
    pub len: usize,
    pub peer: SocketAddr,
    pub at: Instant,
    pub overflow: Option<u32>,
    pub timestamp_missing: bool,
}

pub fn enable(socket: &UdpSocket, kernel: bool) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if kernel {
        use std::os::fd::AsRawFd;
        let enabled: libc::c_int = 1;
        for option in [libc::SO_TIMESTAMPNS, libc::SO_RXQ_OVFL] {
            // SAFETY: valid live socket and a correctly sized c_int pointer.
            let result = unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    option,
                    (&enabled as *const libc::c_int).cast(),
                    std::mem::size_of_val(&enabled) as libc::socklen_t,
                )
            };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (socket, kernel);
    Ok(())
}

pub fn receive(
    socket: &UdpSocket,
    buffer: &mut [u8],
    clock: &Clock,
    kernel: bool,
) -> io::Result<Datagram> {
    if !kernel {
        let (len, peer) = socket.recv_from(buffer)?;
        return Ok(Datagram {
            len,
            peer,
            at: Instant::now(),
            overflow: None,
            timestamp_missing: false,
        });
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: all msghdr buffers live for the complete recvmsg call; the
        // control buffer is word-aligned. Every ancillary length is checked.
        unsafe {
            let mut peer: libc::sockaddr_storage = std::mem::zeroed();
            let mut control = [0usize; 32];
            let mut iov = libc::iovec {
                iov_base: buffer.as_mut_ptr().cast(),
                iov_len: buffer.len(),
            };
            let mut message: libc::msghdr = std::mem::zeroed();
            message.msg_name = (&mut peer as *mut libc::sockaddr_storage).cast();
            message.msg_namelen = std::mem::size_of_val(&peer) as libc::socklen_t;
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = std::mem::size_of_val(&control);
            let length = libc::recvmsg(socket.as_raw_fd(), &mut message, 0);
            let received = Instant::now();
            if length < 0 {
                return Err(io::Error::last_os_error());
            }
            if message.msg_flags & libc::MSG_TRUNC != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "UDP datagram truncated",
                ));
            }
            let mut storage = socket2::SockAddrStorage::zeroed();
            *storage.view_as::<libc::sockaddr_storage>() = peer;
            let peer = socket2::SockAddr::new(storage, message.msg_namelen)
                .as_socket()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Non-IP sender"))?;
            let mut stamp = None;
            let mut overflow = None;
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                let item = &*header;
                if item.cmsg_level == libc::SOL_SOCKET
                    && item.cmsg_type == libc::SCM_TIMESTAMPNS
                    && item.cmsg_len
                        >= libc::CMSG_LEN(std::mem::size_of::<libc::timespec>() as _) as usize
                {
                    let time =
                        std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::timespec>());
                    if time.tv_sec >= 0 && (0..1_000_000_000).contains(&time.tv_nsec) {
                        stamp =
                            clock.stamp(time.tv_sec as i128 * 1_000_000_000 + time.tv_nsec as i128);
                    }
                }
                if item.cmsg_level == libc::SOL_SOCKET
                    && item.cmsg_type == libc::SO_RXQ_OVFL
                    && item.cmsg_len >= libc::CMSG_LEN(4) as usize
                {
                    overflow = Some(std::ptr::read_unaligned(
                        libc::CMSG_DATA(header).cast::<u32>(),
                    ));
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
            Ok(Datagram {
                len: length as usize,
                peer,
                at: stamp.unwrap_or(received),
                overflow,
                timestamp_missing: stamp.is_none() || message.msg_flags & libc::MSG_CTRUNC != 0,
            })
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = clock;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Kernel timestamping requires Linux",
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn kernel_timestamp_is_available_on_loopback() {
        let clock = Clock::default();
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        enable(&receiver, true).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .send_to(b"sample", receiver.local_addr().unwrap())
            .unwrap();
        let mut buffer = [0; 64];
        let result = receive(&receiver, &mut buffer, &clock, true).unwrap();
        assert_eq!(&buffer[..result.len], b"sample");
        assert!(!result.timestamp_missing);
        assert!(Instant::now().duration_since(result.at).as_secs() < 1);
        assert!(!clock.stepped());
    }
}
