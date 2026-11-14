//! Horizon OS Berkeley Sockets (bsd:u, bsd:s) HLE Service.
//!
//! Emulates the Horizon OS BSD socket subsystem over CMIF IPC, providing
//! descriptor management, non-blocking I/O, loopback routing, and sockaddr encoding.

use super::{IpcContext, IpcError, ServiceHandler};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub mod errno {
    pub const ENOENT: u32 = 2;
    pub const EBADF: u32 = 9;
    pub const EAGAIN: u32 = 35; // EWOULDBLOCK
    pub const EWOULDBLOCK: u32 = 35;
    pub const EINPROGRESS: u32 = 36;
    pub const EADDRINUSE: u32 = 48;
    pub const ENETDOWN: u32 = 50;
    pub const ECONNRESET: u32 = 54;
    pub const ENOTCONN: u32 = 57;
    pub const ESHUTDOWN: u32 = 58;
    pub const ECONNREFUSED: u32 = 61;
}

pub const AF_UNSPEC: u32 = 0;
pub const AF_INET: u32 = 2;
pub const AF_INET6: u32 = 10;

pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_RAW: u32 = 3;

pub const IPPROTO_IP: u32 = 0;
pub const IPPROTO_TCP: u32 = 6;
pub const IPPROTO_UDP: u32 = 17;

/// `sockaddr_in` representation (16 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SockAddrIn {
    pub sin_len: u8,
    pub sin_family: u8,
    pub sin_port: u16, // Stored in native host endian for convenience
    pub sin_addr: [u8; 4],
}

impl Default for SockAddrIn {
    fn default() -> Self {
        Self {
            sin_len: 16,
            sin_family: AF_INET as u8,
            sin_port: 0,
            sin_addr: [127, 0, 0, 1],
        }
    }
}

impl SockAddrIn {
    pub fn new(ip: [u8; 4], port: u16) -> Self {
        Self {
            sin_len: 16,
            sin_family: AF_INET as u8,
            sin_port: port,
            sin_addr: ip,
        }
    }

    /// Parses a 16-byte buffer in BSD `sockaddr_in` network byte order.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }
        let sin_len = bytes[0];
        let sin_family = bytes[1];
        // Port is in big-endian network byte order
        let sin_port = u16::from_be_bytes([bytes[2], bytes[3]]);
        let sin_addr = [bytes[4], bytes[5], bytes[6], bytes[7]];

        Some(Self {
            sin_len,
            sin_family,
            sin_port,
            sin_addr,
        })
    }

    /// Serializes into a standard 16-byte BSD network structure.
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0] = self.sin_len;
        buf[1] = self.sin_family;
        let port_bytes = self.sin_port.to_be_bytes();
        buf[2] = port_bytes[0];
        buf[3] = port_bytes[1];
        buf[4..8].copy_from_slice(&self.sin_addr);
        buf
    }
}

/// A buffered network packet in the loopback RX queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub sender: SockAddrIn,
    pub data: Vec<u8>,
}

/// An active guest BSD socket descriptor.
#[derive(Debug)]
pub struct BsdSocket {
    pub fd: i32,
    pub domain: u32,
    pub type_: u32,
    pub protocol: u32,
    pub is_non_blocking: bool,
    pub bound_addr: Option<SockAddrIn>,
    pub connected_addr: Option<SockAddrIn>,
    pub rx_queue: VecDeque<Packet>,
    pub tx_bytes: usize,
    pub rx_bytes: usize,
}

impl BsdSocket {
    pub fn new(fd: i32, domain: u32, type_: u32, protocol: u32) -> Self {
        Self {
            fd,
            domain,
            type_,
            protocol,
            is_non_blocking: false,
            bound_addr: None,
            connected_addr: None,
            rx_queue: VecDeque::new(),
            tx_bytes: 0,
            rx_bytes: 0,
        }
    }
}

/// Thread-safe table of active BSD sockets.
#[derive(Debug, Default)]
pub struct BsdSocketTable {
    pub sockets: HashMap<i32, BsdSocket>,
    pub bound_ports: HashMap<u16, i32>, // UDP port -> socket fd
    pub next_fd: i32,
}

impl BsdSocketTable {
    pub fn new() -> Self {
        Self {
            sockets: HashMap::new(),
            bound_ports: HashMap::new(),
            next_fd: 1,
        }
    }

    pub fn create_socket(
        &mut self,
        domain: u32,
        type_: u32,
        protocol: u32,
    ) -> Result<i32, u32> {
        let fd = self.next_fd;
        self.next_fd += 1;

        let socket = BsdSocket::new(fd, domain, type_, protocol);
        self.sockets.insert(fd, socket);
        Ok(fd)
    }

    pub fn bind(&mut self, fd: i32, addr: SockAddrIn) -> Result<(), u32> {
        if let Some(sock) = self.sockets.get_mut(&fd) {
            if sock.bound_addr.is_some() {
                return Err(errno::EADDRINUSE);
            }
            if sock.type_ == SOCK_DGRAM && addr.sin_port > 0 {
                if self.bound_ports.contains_key(&addr.sin_port) {
                    return Err(errno::EADDRINUSE);
                }
                self.bound_ports.insert(addr.sin_port, fd);
            }
            sock.bound_addr = Some(addr);
            Ok(())
        } else {
            Err(errno::EBADF)
        }
    }

    pub fn connect(&mut self, fd: i32, addr: SockAddrIn) -> Result<(), u32> {
        if let Some(sock) = self.sockets.get_mut(&fd) {
            sock.connected_addr = Some(addr);
            // If UDP, auto-bind if not already bound
            if sock.bound_addr.is_none() {
                let local_port = 40000 + (fd as u16);
                let auto_addr = SockAddrIn::new([127, 0, 0, 1], local_port);
                sock.bound_addr = Some(auto_addr);
                self.bound_ports.insert(local_port, fd);
            }
            Ok(())
        } else {
            Err(errno::EBADF)
        }
    }

    pub fn send_to(
        &mut self,
        fd: i32,
        dest_override: Option<SockAddrIn>,
        data: &[u8],
    ) -> Result<usize, u32> {
        let (src_addr, target_port) = {
            let sock = self.sockets.get_mut(&fd).ok_or(errno::EBADF)?;
            sock.tx_bytes += data.len();
            let src = sock.bound_addr.unwrap_or_else(|| SockAddrIn::new([127, 0, 0, 1], 45000 + fd as u16));
            let dest = dest_override.or(sock.connected_addr).ok_or(errno::ENOTCONN)?;
            (src, dest.sin_port)
        };

        // Loopback packet delivery: check if another socket is bound to target_port
        if let Some(&target_fd) = self.bound_ports.get(&target_port) {
            if let Some(target_sock) = self.sockets.get_mut(&target_fd) {
                target_sock.rx_queue.push_back(Packet {
                    sender: src_addr,
                    data: data.to_vec(),
                });
            }
        }

        Ok(data.len())
    }

    pub fn recv_from(&mut self, fd: i32, max_len: usize) -> Result<Packet, u32> {
        let sock = self.sockets.get_mut(&fd).ok_or(errno::EBADF)?;

        if let Some(packet) = sock.rx_queue.pop_front() {
            let take_len = packet.data.len().min(max_len);
            sock.rx_bytes += take_len;
            Ok(Packet {
                sender: packet.sender,
                data: packet.data[..take_len].to_vec(),
            })
        } else if sock.is_non_blocking {
            Err(errno::EWOULDBLOCK)
        } else {
            // Emulate empty read / pending
            Err(errno::EAGAIN)
        }
    }

    pub fn set_non_blocking(&mut self, fd: i32, non_blocking: bool) -> Result<(), u32> {
        if let Some(sock) = self.sockets.get_mut(&fd) {
            sock.is_non_blocking = non_blocking;
            Ok(())
        } else {
            Err(errno::EBADF)
        }
    }

    pub fn close(&mut self, fd: i32) -> Result<(), u32> {
        if let Some(sock) = self.sockets.remove(&fd) {
            if let Some(addr) = sock.bound_addr {
                self.bound_ports.remove(&addr.sin_port);
            }
            Ok(())
        } else {
            Err(errno::EBADF)
        }
    }
}

/// Horizon BSD Sockets Service (`bsd:u`, `bsd:s`).
pub struct BsdService {
    pub name: String,
    pub socket_table: Arc<Mutex<BsdSocketTable>>,
}

impl Default for BsdService {
    fn default() -> Self {
        Self::new("bsd:u")
    }
}

impl BsdService {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            socket_table: Arc::new(Mutex::new(BsdSocketTable::new())),
        }
    }
}

impl ServiceHandler for BsdService {
    fn service_name(&self) -> &str {
        &self.name
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Cmd 0: Initialize(pid: u64, config...) -> ret: u32, bsd_errno: u32
            0 => {
                context.response.push_u32(0); // ret = 0 (Success)
                context.response.push_u32(0); // bsd_errno = 0
                Ok(())
            }

            // Cmd 2: Socket(domain: u32, type: u32, protocol: u32) -> fd: i32, bsd_errno: u32
            2 => {
                let domain = context.request.read_u32(0).unwrap_or(AF_INET);
                let type_ = context.request.read_u32(4).unwrap_or(SOCK_STREAM);
                let protocol = context.request.read_u32(8).unwrap_or(0);

                let mut table = self.socket_table.lock().unwrap();
                match table.create_socket(domain, type_, protocol) {
                    Ok(fd) => {
                        context.response.push_u32(fd as u32);
                        context.response.push_u32(0); // bsd_errno = 0
                    }
                    Err(err) => {
                        context.response.push_u32(u32::MAX); // -1
                        context.response.push_u32(err);
                    }
                }
                Ok(())
            }

            // Cmd 8: Recv / RecvFrom(fd: i32, flags: u32, max_len: u32)
            8 => {
                let fd = context.request.read_u32(0).unwrap_or(0) as i32;
                let _flags = context.request.read_u32(4).unwrap_or(0);
                let max_len = context.request.read_u32(8).unwrap_or(1500) as usize;

                let mut table = self.socket_table.lock().unwrap();
                match table.recv_from(fd, max_len) {
                    Ok(packet) => {
                        context.response.push_u32(packet.data.len() as u32);
                        context.response.push_u32(0); // errno = 0
                        context.response.push_bytes(&packet.sender.to_bytes());
                        context.response.push_bytes(&packet.data);
                    }
                    Err(err) => {
                        context.response.push_u32(u32::MAX); // -1
                        context.response.push_u32(err);
                        context.response.push_bytes(&[0u8; 16]); // empty sockaddr
                    }
                }
                Ok(())
            }

            // Cmd 9: Send / SendTo(fd: i32, flags: u32, data_len: u32, sockaddr_len: u32, sockaddr: [u8; 16], data: ...)
            9 => {
                let fd = context.request.read_u32(0).unwrap_or(0) as i32;
                let _flags = context.request.read_u32(4).unwrap_or(0);
                let data_len = context.request.read_u32(8).unwrap_or(0) as usize;
                let sockaddr_len = context.request.read_u32(12).unwrap_or(0);

                let (dest_addr, data_offset) = if sockaddr_len >= 8 && context.request.raw_data.len() >= 32 {
                    let addr = SockAddrIn::from_bytes(&context.request.raw_data[16..32]);
                    (addr, 32)
                } else {
                    (None, 16)
                };

                let available = context.request.raw_data.len().saturating_sub(data_offset);
                let to_send = if data_len > 0 {
                    data_len.min(available)
                } else {
                    available
                };

                let data = if data_offset < context.request.raw_data.len() {
                    &context.request.raw_data[data_offset..data_offset + to_send]
                } else {
                    &[]
                };

                let mut table = self.socket_table.lock().unwrap();
                match table.send_to(fd, dest_addr, data) {
                    Ok(sent) => {
                        context.response.push_u32(sent as u32);
                        context.response.push_u32(0); // bsd_errno = 0
                    }
                    Err(err) => {
                        context.response.push_u32(u32::MAX); // -1
                        context.response.push_u32(err);
                    }
                }
                Ok(())
            }

            // Cmd 11: Bind(fd: i32, sockaddr_len: u32, sockaddr: [u8; 16])
            11 => {
                let fd = context.request.read_u32(0).unwrap_or(0) as i32;
                let sockaddr = if context.request.raw_data.len() >= 24 {
                    SockAddrIn::from_bytes(&context.request.raw_data[8..24])
                } else {
                    None
                };

                let mut table = self.socket_table.lock().unwrap();
                if let Some(addr) = sockaddr {
                    match table.bind(fd, addr) {
                        Ok(()) => {
                            context.response.push_u32(0); // ret = 0
                            context.response.push_u32(0); // bsd_errno = 0
                        }
                        Err(err) => {
                            context.response.push_u32(u32::MAX); // -1
                            context.response.push_u32(err);
                        }
                    }
                } else {
                    context.response.push_u32(u32::MAX);
                    context.response.push_u32(errno::EBADF);
                }
                Ok(())
            }

            // Cmd 12: Connect(fd: i32, sockaddr_len: u32, sockaddr: [u8; 16])
            12 => {
                let fd = context.request.read_u32(0).unwrap_or(0) as i32;
                let sockaddr = if context.request.raw_data.len() >= 24 {
                    SockAddrIn::from_bytes(&context.request.raw_data[8..24])
                } else {
                    None
                };

                let mut table = self.socket_table.lock().unwrap();
                if let Some(addr) = sockaddr {
                    match table.connect(fd, addr) {
                        Ok(()) => {
                            context.response.push_u32(0); // ret = 0
                            context.response.push_u32(0); // bsd_errno = 0
                        }
                        Err(err) => {
                            context.response.push_u32(u32::MAX); // -1
                            context.response.push_u32(err);
                        }
                    }
                } else {
                    context.response.push_u32(u32::MAX);
                    context.response.push_u32(errno::EBADF);
                }
                Ok(())
            }

            // Cmd 14: GetSockName(fd: i32) -> ret: u32, bsd_errno: u32, sockaddr: [u8; 16]
            14 => {
                let fd = context.request.read_u32(0).unwrap_or(0) as i32;
                let table = self.socket_table.lock().unwrap();
                if let Some(sock) = table.sockets.get(&fd) {
                    let addr = sock.bound_addr.unwrap_or_default();
                    context.response.push_u32(0);
                    context.response.push_u32(0);
                    context.response.push_bytes(&addr.to_bytes());
                } else {
                    context.response.push_u32(u32::MAX);
                    context.response.push_u32(errno::EBADF);
                    context.response.push_bytes(&[0u8; 16]);
                }
                Ok(())
            }

            // Cmd 20: Close(fd: i32) -> ret: u32, bsd_errno: u32
            20 => {
                let fd = context.request.read_u32(0).unwrap_or(0) as i32;
                let mut table = self.socket_table.lock().unwrap();
                match table.close(fd) {
                    Ok(()) => {
                        context.response.push_u32(0);
                        context.response.push_u32(0);
                    }
                    Err(err) => {
                        context.response.push_u32(u32::MAX);
                        context.response.push_u32(err);
                    }
                }
                Ok(())
            }

            // Cmd 26: Fcntl(fd: i32, cmd: u32, arg: u32) -> ret: u32, bsd_errno: u32
            26 => {
                let fd = context.request.read_u32(0).unwrap_or(0) as i32;
                let cmd = context.request.read_u32(4).unwrap_or(0);
                let arg = context.request.read_u32(8).unwrap_or(0);

                let mut table = self.socket_table.lock().unwrap();
                match cmd {
                    // F_GETFL (3)
                    3 => {
                        if let Some(sock) = table.sockets.get(&fd) {
                            let flags = if sock.is_non_blocking { 0x800 } else { 0 };
                            context.response.push_u32(flags);
                            context.response.push_u32(0);
                        } else {
                            context.response.push_u32(u32::MAX);
                            context.response.push_u32(errno::EBADF);
                        }
                    }
                    // F_SETFL (4)
                    4 => {
                        let non_blocking = (arg & 0x800) != 0 || (arg & 0x04) != 0;
                        match table.set_non_blocking(fd, non_blocking) {
                            Ok(()) => {
                                context.response.push_u32(0);
                                context.response.push_u32(0);
                            }
                            Err(err) => {
                                context.response.push_u32(u32::MAX);
                                context.response.push_u32(err);
                            }
                        }
                    }
                    _ => {
                        context.response.push_u32(0);
                        context.response.push_u32(0);
                    }
                }
                Ok(())
            }

            other => Err(IpcError::UnknownCommand(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ipc::cmif::CmifRequest;

    #[test]
    fn test_bsd_socket_udp_loopback_send_recv() {
        let bsd = BsdService::new("bsd:u");

        // 1. Create UDP Server Socket
        let server_fd = {
            let mut table = bsd.socket_table.lock().unwrap();
            table.create_socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP).unwrap()
        };
        assert_eq!(server_fd, 1);

        // Bind server to port 8080
        let server_addr = SockAddrIn::new([127, 0, 0, 1], 8080);
        {
            let mut table = bsd.socket_table.lock().unwrap();
            table.bind(server_fd, server_addr).unwrap();
        }

        // 2. Create UDP Client Socket
        let client_fd = {
            let mut table = bsd.socket_table.lock().unwrap();
            table.create_socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP).unwrap()
        };
        assert_eq!(client_fd, 2);

        // 3. Client sends datagram to Server port 8080
        let test_payload = b"PING_FROM_MOCKRA_KAGAMI";
        {
            let mut table = bsd.socket_table.lock().unwrap();
            let sent = table.send_to(client_fd, Some(server_addr), test_payload).unwrap();
            assert_eq!(sent, test_payload.len());
        }

        // 4. Server receives datagram
        {
            let mut table = bsd.socket_table.lock().unwrap();
            let received = table.recv_from(server_fd, 1024).unwrap();
            assert_eq!(received.data, test_payload);
            assert_eq!(received.sender.sin_port, 45000 + client_fd as u16);
        }

        // 5. Server responds back to Client
        let reply_payload = b"PONG_SUCCESS";
        let client_addr = SockAddrIn::new([127, 0, 0, 1], 45000 + client_fd as u16);
        {
            let mut table = bsd.socket_table.lock().unwrap();
            table.bind(client_fd, client_addr).unwrap();
            let sent = table.send_to(server_fd, Some(client_addr), reply_payload).unwrap();
            assert_eq!(sent, reply_payload.len());
        }

        // 6. Client receives reply
        {
            let mut table = bsd.socket_table.lock().unwrap();
            let received = table.recv_from(client_fd, 1024).unwrap();
            assert_eq!(received.data, reply_payload);
            assert_eq!(received.sender.sin_port, 8080);
        }
    }

    #[test]
    fn test_bsd_non_blocking_and_fcntl() {
        let bsd = BsdService::new("bsd:u");

        // Create socket
        let fd = {
            let mut table = bsd.socket_table.lock().unwrap();
            table.create_socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP).unwrap()
        };

        // By default, recv on empty queue returns EAGAIN
        {
            let mut table = bsd.socket_table.lock().unwrap();
            assert_eq!(table.recv_from(fd, 100), Err(errno::EAGAIN));
        }

        // Set non-blocking via Fcntl (Cmd 26)
        let mut fcntl_buf = [0u8; 16 + 12];
        fcntl_buf[0..4].copy_from_slice(b"SFCI");
        fcntl_buf[8..12].copy_from_slice(&26u32.to_le_bytes()); // Cmd 26: Fcntl
        fcntl_buf[16..20].copy_from_slice(&(fd as u32).to_le_bytes());
        fcntl_buf[20..24].copy_from_slice(&4u32.to_le_bytes()); // F_SETFL
        fcntl_buf[24..28].copy_from_slice(&0x800u32.to_le_bytes()); // O_NONBLOCK

        let mut ctx = IpcContext::new(CmifRequest::parse(&fcntl_buf).unwrap());
        bsd.dispatch(&mut ctx).unwrap();
        assert_eq!(ctx.response.result_code, 0);

        // Now recv returns EWOULDBLOCK (35)
        {
            let mut table = bsd.socket_table.lock().unwrap();
            assert_eq!(table.recv_from(fd, 100), Err(errno::EWOULDBLOCK));
        }
    }
}
