use emu_core::cpu::{Cpu, StepResult};
use emu_core::kernel::{KernelContext, SvcOutcome};
use emu_core::loader::load_nro;
use emu_core::memory::{MemoryBus, SystemBus};
use std::path::PathBuf;

#[test]
fn test_nro_execution_with_horizon_svcs() {
    let mut bus = SystemBus::new();

    // Standard Switch 64-bit Address Space Regions
    const LOAD_BASE: u64 = 0x710000_0000;
    const BINARY_MEM_SIZE: usize = 0x10_0000; // 1 MB code region
    const HEAP_BASE: u64 = 0x8800_0000;
    const HEAP_SIZE: usize = 0x40_0000;       // 4 MB heap region

    bus.map_ram(LOAD_BASE, BINARY_MEM_SIZE);
    bus.map_ram(HEAP_BASE, HEAP_SIZE);

    let rom_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/test_roms/hello_horizon.nro");

    let header = load_nro(&mut bus, &rom_path, LOAD_BASE)
        .expect("Failed to load hello_horizon.nro");

    assert_eq!(&header.magic, b"NRO0");

    let mut kernel = KernelContext::new(false);
    let mut cpu = Cpu::new();
    cpu.reset(LOAD_BASE);

    let mut cycles = 0;
    const MAX_CYCLES: u64 = 10_000;
    let mut process_exited = false;

    while cycles < MAX_CYCLES && !process_exited {
        cycles += 1;
        match cpu.step(&mut bus).expect("CPU execution error") {
            StepResult::Continue => continue,
            StepResult::Svc(svc_id) => {
                let outcome = kernel
                    .handle_svc(svc_id, &mut cpu, &mut bus)
                    .expect("SVC handling error");
                if outcome == SvcOutcome::ExitProcess {
                    process_exited = true;
                }
            }
            StepResult::Halt | StepResult::Breakpoint => break,
        }
    }

    assert!(process_exited, "NRO did not cleanly exit via svcExitProcess");

    // 1. Verify debug log from svcOutputDebugString
    let output = kernel.get_debug_log();
    assert_eq!(output, "Hello Horizon OS from NRO!\n");

    // 2. Verify heap allocation from svcSetHeapSize
    assert_eq!(kernel.process.heap_size, 0x20_0000);

    // 3. Verify memory write to the allocated heap base
    let written_heap_val = bus.read_u32(HEAP_BASE).unwrap();
    assert_eq!(written_heap_val, 0xBEEF);
}

#[test]
fn test_nro_codecave_patching() {
    let mut bus = SystemBus::new();
    const LOAD_BASE: u64 = 0x710000_0000;
    const HEAP_BASE: u64 = 0x8800_0000;
    bus.map_ram(LOAD_BASE, 0x10_0000);
    bus.map_ram(HEAP_BASE, 0x40_0000);

    let rom_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/test_roms/hello_horizon.nro");

    load_nro(&mut bus, &rom_path, LOAD_BASE).unwrap();

    let mut kernel = KernelContext::new(false);

    // Dynamic patch: patch the string in .rodata (offset 0x200) from "Hello..." to "Patched..."
    // Offset 0x200: "Patched Horizon OS!"
    let patched_text = b"Patched Horizon OS!\n";
    kernel.patch_manager.add_patch(emu_core::kernel::MemoryPatch::new(
        "Horizon Banner Override",
        0x200,
        patched_text.to_vec(),
    ));

    // Also patch length in main: at offset 0x88 (instruction: movz x1, #27 -> movz x1, #20)
    // In ARM64: MOVZ X1, #20 is 0xD2800281 (little-endian: 81 02 80 D2)
    kernel.patch_manager
        .add_hex_patch("Update String Length", 0x84, "81 02 80 D2")
        .unwrap();

    let applied = kernel.patch_manager.apply_all(&mut bus, LOAD_BASE).unwrap();
    assert_eq!(applied, 2);

    let mut cpu = Cpu::new();
    cpu.reset(LOAD_BASE);

    let mut cycles = 0;
    while cycles < 1000 {
        cycles += 1;
        match cpu.step(&mut bus).unwrap() {
            StepResult::Continue => continue,
            StepResult::Svc(svc_id) => {
                if kernel.handle_svc(svc_id, &mut cpu, &mut bus).unwrap() == SvcOutcome::ExitProcess {
                    break;
                }
            }
            _ => break,
        }
    }

    assert_eq!(kernel.get_debug_log(), "Patched Horizon OS!\n");
}
