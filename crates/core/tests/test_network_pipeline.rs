//! End-to-end integration tests for Horizon OS Network Pipeline:
//! Network Interface Manager (nifm:u), BSD Sockets (bsd:u), and CMIF IPC Dispatch.

use emu_core::cpu::Cpu;
use emu_core::kernel::KernelContext;
use emu_core::memory::fastmem::{permissions, Fastmem};
use emu_core::memory::MemoryBus;

fn write_sfci_cmd(mem: &mut impl MemoryBus, tls_addr: u64, cmd: u32) {
    mem.write_u8(tls_addr + 0, b'S').unwrap();
    mem.write_u8(tls_addr + 1, b'F').unwrap();
    mem.write_u8(tls_addr + 2, b'C').unwrap();
    mem.write_u8(tls_addr + 3, b'I').unwrap();
    mem.write_u32(tls_addr + 4, 0).unwrap();
    mem.write_u32(tls_addr + 8, cmd).unwrap();
    mem.write_u32(tls_addr + 12, 0).unwrap();
}

#[test]
fn test_nifm_service_ipc_and_async_request_flow() {
    let mut mem = Fastmem::new();
    let tls_addr = 0x8000_1000;
    mem.map_range(0x8000_0000, 0x10000, permissions::RWX);

    let mut cpu = Cpu::new();
    cpu.reset(0x8000_0000);
    let mut kernel = KernelContext::new(false);
    kernel.tls_base = tls_addr;

    let sm_handle = kernel.sm_session_handle;

    // 1. Request "nifm:u" from ServiceManager (sm:)
    write_sfci_cmd(&mut mem, tls_addr, 1); // Cmd 1: GetService
    for (i, &b) in b"nifm:u\0\0".iter().take(8).enumerate() {
        mem.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0); // ResultSuccess

    let nifm_handle = mem.read_u32(tls_addr + 16).unwrap();
    assert_ne!(nifm_handle, 0);

    // 2. nifm:u -> CreateGeneralService (Cmd 4)
    write_sfci_cmd(&mut mem, tls_addr, 4);
    cpu.regs.set_x(0, nifm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let general_handle = mem.read_u32(tls_addr + 16).unwrap();
    assert_ne!(general_handle, 0);

    // 3. IGeneralService -> GetCurrentIpAddress (Cmd 12)
    write_sfci_cmd(&mut mem, tls_addr, 12);
    cpu.regs.set_x(0, general_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let ip_val = mem.read_u32(tls_addr + 16).unwrap();
    assert_eq!(ip_val.to_le_bytes(), [192, 168, 1, 105]);

    // 4. IGeneralService -> CreateRequest (Cmd 4)
    write_sfci_cmd(&mut mem, tls_addr, 4);
    mem.write_u32(tls_addr + 16, 1).unwrap(); // requirement preset = 1
    cpu.regs.set_x(0, general_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let request_handle = mem.read_u32(tls_addr + 16).unwrap();
    assert_ne!(request_handle, 0);

    // 5. IRequest -> GetSystemEventReadableHandle (Cmd 2)
    write_sfci_cmd(&mut mem, tls_addr, 2);
    cpu.regs.set_x(0, request_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let event_handle = mem.read_u32(tls_addr + 16).unwrap();
    assert_ne!(event_handle, 0);

    // 6. IRequest -> Submit (Cmd 4) -> signals the completion event
    write_sfci_cmd(&mut mem, tls_addr, 4);
    cpu.regs.set_x(0, request_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    // 7. Verify completion event is signaled via svcWaitSynchronization (0x18)
    cpu.regs.set_x(1, event_handle as u64);
    cpu.regs.set_w(2, 1); // 1 handle
    kernel.handle_svc(0x18, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0); // ResultSuccess
    assert_eq!(cpu.regs.get_x(1), 0); // Handle at index 0 signaled
}

#[test]
fn test_bsd_service_socket_ipc_and_loopback_traffic() {
    let mut mem = Fastmem::new();
    let tls_addr = 0x8000_2000;
    mem.map_range(0x8000_0000, 0x10000, permissions::RWX);

    let mut cpu = Cpu::new();
    cpu.reset(0x8000_0000);
    let mut kernel = KernelContext::new(false);
    kernel.tls_base = tls_addr;

    let sm_handle = kernel.sm_session_handle;

    // 1. Request "bsd:u" from ServiceManager (sm:)
    write_sfci_cmd(&mut mem, tls_addr, 1);
    for (i, &b) in b"bsd:u\0\0\0".iter().take(8).enumerate() {
        mem.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let bsd_handle = mem.read_u32(tls_addr + 16).unwrap();
    assert_ne!(bsd_handle, 0);

    // 2. bsd:u -> Initialize (Cmd 0)
    write_sfci_cmd(&mut mem, tls_addr, 0);
    cpu.regs.set_x(0, bsd_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
    assert_eq!(mem.read_u32(tls_addr + 16).unwrap(), 0); // ret = 0

    // 3. Create Server UDP Socket (Cmd 2: Socket)
    write_sfci_cmd(&mut mem, tls_addr, 2);
    mem.write_u32(tls_addr + 16, 2).unwrap(); // domain: AF_INET (2)
    mem.write_u32(tls_addr + 20, 2).unwrap(); // type: SOCK_DGRAM (2)
    mem.write_u32(tls_addr + 24, 17).unwrap(); // protocol: IPPROTO_UDP (17)

    cpu.regs.set_x(0, bsd_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
    let server_fd = mem.read_u32(tls_addr + 16).unwrap() as i32;
    assert_eq!(server_fd, 1);

    // 4. Bind Server Socket to port 9000 (Cmd 11: Bind)
    write_sfci_cmd(&mut mem, tls_addr, 11);
    mem.write_u32(tls_addr + 16, server_fd as u32).unwrap();
    mem.write_u32(tls_addr + 20, 16).unwrap(); // sockaddr_len = 16
    // sockaddr: [len: 16, family: 2, port: 9000 (be: 0x23, 0x28), ip: 127.0.0.1]
    mem.write_u8(tls_addr + 24, 16).unwrap();
    mem.write_u8(tls_addr + 25, 2).unwrap();
    mem.write_u8(tls_addr + 26, 0x23).unwrap();
    mem.write_u8(tls_addr + 27, 0x28).unwrap();
    mem.write_u8(tls_addr + 28, 127).unwrap();
    mem.write_u8(tls_addr + 29, 0).unwrap();
    mem.write_u8(tls_addr + 30, 0).unwrap();
    mem.write_u8(tls_addr + 31, 1).unwrap();

    cpu.regs.set_x(0, bsd_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
    assert_eq!(mem.read_u32(tls_addr + 16).unwrap(), 0); // ret = 0

    // 5. Create Client UDP Socket (Cmd 2: Socket)
    write_sfci_cmd(&mut mem, tls_addr, 2);
    mem.write_u32(tls_addr + 16, 2).unwrap();
    mem.write_u32(tls_addr + 20, 2).unwrap();
    mem.write_u32(tls_addr + 24, 17).unwrap();

    cpu.regs.set_x(0, bsd_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
    let client_fd = mem.read_u32(tls_addr + 16).unwrap() as i32;
    assert_eq!(client_fd, 2);

    // 6. Client sends datagram to Server port 9000 (Cmd 9: SendTo)
    let payload = b"STARVALD_NETWORK_PING";
    write_sfci_cmd(&mut mem, tls_addr, 9);
    mem.write_u32(tls_addr + 16, client_fd as u32).unwrap();
    mem.write_u32(tls_addr + 20, 0).unwrap(); // flags = 0
    mem.write_u32(tls_addr + 24, payload.len() as u32).unwrap(); // data_len
    mem.write_u32(tls_addr + 28, 16).unwrap(); // sockaddr_len = 16
    // target sockaddr: port 9000 (at offset +32)
    mem.write_u8(tls_addr + 32, 16).unwrap();
    mem.write_u8(tls_addr + 33, 2).unwrap();
    mem.write_u8(tls_addr + 34, 0x23).unwrap();
    mem.write_u8(tls_addr + 35, 0x28).unwrap();
    mem.write_u8(tls_addr + 36, 127).unwrap();
    mem.write_u8(tls_addr + 37, 0).unwrap();
    mem.write_u8(tls_addr + 38, 0).unwrap();
    mem.write_u8(tls_addr + 39, 1).unwrap();
    // payload starts at +48
    for (i, &b) in payload.iter().enumerate() {
        mem.write_u8(tls_addr + 48 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, bsd_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
    let bytes_sent = mem.read_u32(tls_addr + 16).unwrap();
    assert_eq!(bytes_sent as usize, payload.len());

    // 7. Server receives datagram (Cmd 8: RecvFrom)
    write_sfci_cmd(&mut mem, tls_addr, 8);
    mem.write_u32(tls_addr + 16, server_fd as u32).unwrap();
    mem.write_u32(tls_addr + 20, 0).unwrap(); // flags = 0
    mem.write_u32(tls_addr + 24, 1024).unwrap(); // max_len = 1024

    cpu.regs.set_x(0, bsd_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let bytes_received = mem.read_u32(tls_addr + 16).unwrap();
    assert_eq!(bytes_received as usize, payload.len());

    // Read received payload (offset +40 in response: +16 header, +4 ret, +4 errno, +16 sender sockaddr)
    let mut recv_data = vec![0u8; bytes_received as usize];
    for (i, slot) in recv_data.iter_mut().enumerate() {
        *slot = mem.read_u8(tls_addr + 40 + i as u64).unwrap();
    }
    assert_eq!(&recv_data, payload);

    // 8. Close sockets (Cmd 20: Close)
    write_sfci_cmd(&mut mem, tls_addr, 20);
    mem.write_u32(tls_addr + 16, server_fd as u32).unwrap();
    cpu.regs.set_x(0, bsd_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
}
