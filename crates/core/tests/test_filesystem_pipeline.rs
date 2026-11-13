use emu_core::cpu::Cpu;
use emu_core::fs::{
    FileMode, FileSystemProxyService, HostFileSystem, IFileSystem, Pfs0Archive, Pfs0Builder,
    RomfsBuilder,
};
use emu_core::kernel::KernelContext;
use emu_core::memory::{FlatMemory, MemoryBus};
use std::env;
use std::sync::Arc;

#[test]
fn test_pfs0_archive_nsp_extraction() {
    let mut builder = Pfs0Builder::new();
    builder.add_file("main", b"\x7FELF_AARCH64_EXECUTABLE_BINARY".to_vec());
    builder.add_file("main.npdm", b"META_NPDM_PROGRAM_INFO".to_vec());
    builder.add_file("icon.jpg", vec![0xFF, 0xD8, 0xFF, 0xE0]);

    let nsp_bytes = builder.build();
    let archive = Pfs0Archive::parse(&nsp_bytes).expect("PFS0/NSP parsing failed");

    assert_eq!(archive.files().len(), 3);
    assert_eq!(archive.header().file_count, 3);

    let main_bin = archive.get_file_data("main").unwrap();
    assert_eq!(main_bin, b"\x7FELF_AARCH64_EXECUTABLE_BINARY");

    let file = archive.open_file("/main.npdm", FileMode::Read).unwrap();
    assert_eq!(file.get_size().unwrap(), 22);
}

#[test]
fn test_fsp_srv_ipc_romfs_and_save_data_flow() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new(0, 0x20000);
    let mut kernel = KernelContext::new(false);

    // 1. Build sample RomFS asset tree
    let mut romfs_builder = RomfsBuilder::new();
    romfs_builder.add_file("/levels/world1.bin", b"WORLD_STAGE_1_GEOMETRY".to_vec());
    romfs_builder.add_file("/scripts/init.lua", b"print('Hello Horizon!')".to_vec());
    let romfs = Arc::new(romfs_builder.build());

    // 2. Setup HostFileSystem for user save data in temporary directory
    let temp_save_dir = env::temp_dir().join("switch_emu_integration_save_dir");
    let save_fs = Arc::new(HostFileSystem::new(&temp_save_dir).unwrap());

    // 3. Register custom fsp-srv configured with this application's RomFS & SaveData
    let fsp = Arc::new(
        FileSystemProxyService::new()
            .with_romfs(romfs)
            .with_save_data(save_fs),
    );
    kernel.service_manager.register_factory("fsp-srv", move || fsp.clone());

    let sm_handle = kernel.sm_session_handle;
    let tls_addr = 0x1000;
    kernel.tls_base = tls_addr;

    // Step A: Request "fsp-srv" from ServiceManager (sm:)
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 1).unwrap(); // Cmd 1: GetService
    for (i, &b) in b"fsp-srv\0".iter().take(8).enumerate() {
        bus.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let fsp_handle = bus.read_u32(tls_addr + 16).unwrap();
    assert_ne!(fsp_handle, 0);

    // Step B: MountRomfs (Cmd 0) on fsp-srv
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 0).unwrap(); // Cmd 0: MountRomfs

    cpu.regs.set_x(0, fsp_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let romfs_fs_handle = bus.read_u32(tls_addr + 16).unwrap();
    assert_ne!(romfs_fs_handle, 0);

    // Step C: OpenFile("/levels/world1.bin") on RomFS IFileSystem
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 0).unwrap(); // Cmd 0: OpenFile
    bus.write_u32(tls_addr + 16, 1).unwrap(); // mode: Read (1)
    for (i, &b) in b"/levels/world1.bin\0".iter().enumerate() {
        bus.write_u8(tls_addr + 20 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, romfs_fs_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let level_file_handle = bus.read_u32(tls_addr + 16).unwrap();
    assert_ne!(level_file_handle, 0);

    // Step D: Read from the level file (Cmd 0 on IFile)
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 0).unwrap(); // Cmd 0: Read
    bus.write_u64(tls_addr + 16, 0).unwrap(); // offset: 0
    bus.write_u64(tls_addr + 24, 22).unwrap(); // size: 22

    cpu.regs.set_x(0, level_file_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let bytes_read = bus.read_u64(tls_addr + 16).unwrap();
    assert_eq!(bytes_read, 22);

    let mut asset_data = [0u8; 22];
    for (i, b) in asset_data.iter_mut().enumerate() {
        *b = bus.read_u8(tls_addr + 24 + i as u64).unwrap();
    }
    assert_eq!(&asset_data, b"WORLD_STAGE_1_GEOMETRY");

    // Cleanup temporary save directory
    let _ = std::fs::remove_dir_all(&temp_save_dir);
}
