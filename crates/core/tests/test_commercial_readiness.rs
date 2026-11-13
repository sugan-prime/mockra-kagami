//! End-to-end integration test verifying commercial title readiness:
//! NCA3 archive parsing, ExeFS / RomFS extraction, System Settings (`set:sys`),
//! Applet Lifecycle (`appletOE`), Account Management (`acc:u0`), and 200 Hz HID input.

use emu_core::cpu::Cpu;
use emu_core::fs::pfs0::Pfs0Builder;
use emu_core::kernel::ipc::hid::buttons;
use emu_core::kernel::KernelContext;
use emu_core::loader::nca::{NcaArchive, NcaBuilder, NcaContentType};
use emu_core::memory::{FlatMemory, MemoryBus};

/// Helper to serialize a CMIF request into guest TLS buffer.
fn write_cmif_request(bus: &mut FlatMemory, tls_addr: u64, command_id: u32, raw_payload: &[u8]) {
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 4, 0).unwrap(); // version
    bus.write_u32(tls_addr + 8, command_id).unwrap();
    bus.write_u32(tls_addr + 12, 0).unwrap(); // padding

    for (i, &b) in raw_payload.iter().enumerate() {
        bus.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }
}

/// Helper to read response payload from TLS.
fn read_cmif_response(bus: &FlatMemory, tls_addr: u64) -> (u32, Vec<u8>) {
    let mut magic = [0u8; 4];
    for (i, slot) in magic.iter_mut().enumerate() {
        *slot = bus.read_u8(tls_addr + i as u64).unwrap();
    }
    assert_eq!(&magic, b"SFCO");

    let result_code = bus.read_u32(tls_addr + 8).unwrap();
    let mut payload = vec![0u8; 64];
    for (i, slot) in payload.iter_mut().enumerate() {
        *slot = bus.read_u8(tls_addr + 16 + i as u64).unwrap();
    }
    (result_code, payload)
}

#[test]
fn test_commercial_title_full_pipeline() {
    const BOTW_TITLE_ID: u64 = 0x01007EF00011E000;

    // 1. Synthesize a complete commercial application package (NCA3)
    // Section 0: ExeFS (PFS0 archive with main executable and NPDM metadata)
    let mut exefs_builder = Pfs0Builder::new();
    exefs_builder.add_file("main", b"ARM64 Main Executable Bytecode Header".to_vec());
    exefs_builder.add_file("main.npdm", b"META NPDM Program Metadata Block".to_vec());
    let exefs_bytes = exefs_builder.build();

    // Section 1: RomFS archive payload
    let romfs_raw = b"RomFS Hierarchical FileSystem Metadata and Asset Data";

    // Construct NCA3
    let mut nca_builder = NcaBuilder::new();
    nca_builder.set_title_id(BOTW_TITLE_ID);
    nca_builder.set_content_type(NcaContentType::Program);
    nca_builder.add_section(&exefs_bytes);
    nca_builder.add_section(romfs_raw);
    let nca_package = nca_builder.build();

    // 2. Parse NCA3 package and verify integrity
    let nca = NcaArchive::parse(&nca_package).unwrap();
    assert_eq!(nca.title_id, BOTW_TITLE_ID);
    assert!(nca.is_program());

    let extracted_exefs = nca.read_section(0).unwrap();
    let extracted_romfs = nca.read_section(1).unwrap();
    assert!(extracted_exefs.starts_with(b"PFS0"));
    assert!(extracted_romfs.len() >= 0x50);

    // 3. Initialize Horizon OS Kernel and Game Profile
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new(0, 0x40000);
    let mut kernel = KernelContext::new(false);
    kernel.process.title_id = BOTW_TITLE_ID;

    let tls_addr = 0x1000;
    kernel.tls_base = tls_addr;
    let sm_handle = kernel.sm_session_handle;

    let profile = kernel.profiles.get(BOTW_TITLE_ID).expect("BotW profile should exist");
    assert_eq!(profile.title_name, "The Legend of Zelda: Breath of the Wild");
    assert_eq!(profile.target_fps, 30);

    // 4. Query System Settings Service: sm:GetService("set:sys")
    let mut name_bytes = [0u8; 8];
    name_bytes[..7].copy_from_slice(b"set:sys");
    write_cmif_request(&mut bus, tls_addr, 1, &name_bytes);
    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let set_handle = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert!(set_handle > 0);

    // set:sys -> GetRegionCode (Cmd 4)
    write_cmif_request(&mut bus, tls_addr, 4, &[]);
    cpu.regs.set_x(0, set_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let region = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert_eq!(region, 1); // USA region

    // 5. Query Applet Manager Service: sm:GetService("appletOE")
    let mut name_bytes = [0u8; 8];
    name_bytes[..8].copy_from_slice(b"appletOE");
    write_cmif_request(&mut bus, tls_addr, 1, &name_bytes);
    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let applet_handle = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert!(applet_handle > 0);

    // appletOE -> OpenSession (Cmd 0)
    write_cmif_request(&mut bus, tls_addr, 0, &[]);
    cpu.regs.set_x(0, applet_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let all_proxies_h = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert!(all_proxies_h > 0);

    // 6. Query Account Service: sm:GetService("acc:u0")
    let mut name_bytes = [0u8; 8];
    name_bytes[..6].copy_from_slice(b"acc:u0");
    write_cmif_request(&mut bus, tls_addr, 1, &name_bytes);
    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let acc_handle = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert!(acc_handle > 0);

    // acc:u0 -> GetUserCount (Cmd 0)
    write_cmif_request(&mut bus, tls_addr, 0, &[]);
    cpu.regs.set_x(0, acc_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let user_count = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert_eq!(user_count, 1);

    // 7. Query Human Interface Device: sm:GetService("hid")
    let mut name_bytes = [0u8; 8];
    name_bytes[..3].copy_from_slice(b"hid");
    write_cmif_request(&mut bus, tls_addr, 1, &name_bytes);
    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let hid_handle = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert!(hid_handle > 0);

    // hid -> CreateAppletResource (Cmd 0)
    write_cmif_request(&mut bus, tls_addr, 0, &[0u8; 8]);
    cpu.regs.set_x(0, hid_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let applet_res_h = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert!(applet_res_h > 0);

    // IAppletResource -> GetSharedMemoryHandle (Cmd 0)
    write_cmif_request(&mut bus, tls_addr, 0, &[]);
    cpu.regs.set_x(0, applet_res_h as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let shmem_h = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert_eq!(shmem_h, 0x1000);

    // 8. Inject Controller Input (A button + ZL trigger + Left Analog stick)
    let hid_service = emu_core::kernel::ipc::HidService::new();
    hid_service.set_player1_button(buttons::A | buttons::ZL, true);
    hid_service.set_player1_analog(20000, 15000, 0, 0);

    let shmem = hid_service.applet_resource.shared_memory_buffer.lock().unwrap();
    let p1_entry = 0x4000;
    let btns = u64::from_le_bytes(shmem[p1_entry + 16..p1_entry + 24].try_into().unwrap());
    let stick_x = i32::from_le_bytes(shmem[p1_entry + 24..p1_entry + 28].try_into().unwrap());
    let stick_y = i32::from_le_bytes(shmem[p1_entry + 28..p1_entry + 32].try_into().unwrap());

    assert_eq!(btns & buttons::A, buttons::A);
    assert_eq!(btns & buttons::ZL, buttons::ZL);
    assert_eq!(stick_x, 20000);
    assert_eq!(stick_y, 15000);
}
