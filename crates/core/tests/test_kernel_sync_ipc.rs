use emu_core::cpu::Cpu;
use emu_core::kernel::KernelContext;
use emu_core::memory::{FlatMemory, MemoryBus};

#[test]
fn test_game_profile_application_botw() {
    let mut kernel = KernelContext::new(false);
    kernel.process.title_id = 0x01007EF00011E000; // Zelda: Breath of the Wild

    let profile = kernel.profiles.get(kernel.process.title_id).expect("BotW profile should exist");
    assert_eq!(profile.title_name, "The Legend of Zelda: Breath of the Wild");
    assert_eq!(profile.target_fps, 30);
    assert_eq!(profile.default_heap_size, 0x4000_0000); // 1 GB
    assert_eq!(profile.properties.get("GPUBufferCacheAccuracy").unwrap(), "High");

    // Apply profile heap configuration to process
    kernel.process.heap_size = profile.default_heap_size;
    assert_eq!(kernel.process.heap_size, 0x4000_0000);
}

#[test]
fn test_kernel_event_synchronization_lifecycle() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new(0, 0x10000);
    let mut kernel = KernelContext::new(false);

    // 1. Create event with auto_reset = true (SVC 0x45)
    cpu.regs.set_x(1, 1);
    kernel.handle_svc(0x45, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
    let writable_handle = cpu.regs.get_w(1);
    let readable_handle = cpu.regs.get_w(2);

    // 2. Wait on event when unsignaled -> expects timeout (0xEA01)
    cpu.regs.set_x(1, readable_handle as u64);
    cpu.regs.set_w(2, 1); // 1 handle
    cpu.regs.set_x(3, 0); // 0ns timeout
    kernel.handle_svc(0x18, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_w(0), 0xEA01);

    // 3. Signal event using writable handle (SVC 0x1E)
    cpu.regs.set_x(0, writable_handle as u64);
    kernel.handle_svc(0x1E, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    // 4. Wait again on readable handle -> expects immediate success
    cpu.regs.set_x(1, readable_handle as u64);
    cpu.regs.set_w(2, 1);
    kernel.handle_svc(0x18, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0); // Success
    assert_eq!(cpu.regs.get_x(1), 0); // Handle index 0

    // 5. Subsequent wait should timeout because auto_reset consumed the signal
    kernel.handle_svc(0x18, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_w(0), 0xEA01);

    // 6. Signal and manually clear (SVC 0x1F)
    cpu.regs.set_x(0, writable_handle as u64);
    kernel.handle_svc(0x1E, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    cpu.regs.set_x(0, writable_handle as u64);
    kernel.handle_svc(0x1F, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    // Wait must now timeout because it was manually cleared
    cpu.regs.set_x(1, readable_handle as u64);
    cpu.regs.set_w(2, 1);
    kernel.handle_svc(0x18, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_w(0), 0xEA01);
}

#[test]
fn test_kernel_ipc_service_manager_and_hid() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new(0, 0x10000);
    let mut kernel = KernelContext::new(false);

    let sm_handle = kernel.sm_session_handle;
    let tls_addr = 0x1000;
    kernel.tls_base = tls_addr;

    // 1. Request "hid" service from sm: (Cmd 1: GetService)
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 1).unwrap(); // Cmd 1: GetService
    for (i, &b) in b"hid\0\0\0\0\0".iter().take(8).enumerate() {
        bus.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0); // Success

    let hid_handle = bus.read_u32(tls_addr + 16).unwrap();
    assert_ne!(hid_handle, 0);

    // 2. Dispatch command to HID service: Cmd 103 (ActivateNpad)
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 103).unwrap(); // Cmd 103: ActivateNpad

    cpu.regs.set_x(0, hid_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0); // Success
}
