use emu_core::cpu::{Cpu, StepResult};
use emu_core::kernel::{KernelContext, SvcOutcome};
use emu_core::loader::{load_flat_binary, load_nro, NRO_MAGIC};
use emu_core::memory::{DebugUart, SystemBus};
use std::env;
use std::fs;
use std::path::Path;

const BAREMETAL_RAM_BASE: u64 = 0x8000_0000;
const BAREMETAL_RAM_SIZE: usize = 0x10_0000; // 1 MB
const UART_BASE: u64 = 0xD000_0000;
const UART_SIZE: u64 = 0x1000;

const HORIZON_LOAD_BASE: u64 = 0x710000_0000;
const HORIZON_CODE_SIZE: usize = 0x10_0000;  // 1 MB
const HORIZON_HEAP_BASE: u64 = 0x8800_0000;
const HORIZON_HEAP_SIZE: usize = 0x40_0000;  // 4 MB

fn main() {
    println!("=== Switch-Emulator (Rust AArch64 & Horizon OS HLE Runner) ===\n");

    let args: Vec<String> = env::args().collect();
    let rom_path = if args.len() > 1 {
        args[1].clone()
    } else {
        let default_nro = "tests/test_roms/hello_horizon.nro";
        let default_bin = "tests/test_roms/hello_uart.bin";
        if Path::new(default_nro).exists() {
            println!("No ROM path provided. Running default Horizon NRO: {}", default_nro);
            default_nro.to_string()
        } else if Path::new(default_bin).exists() {
            println!("No ROM path provided. Running default bare-metal ROM: {}", default_bin);
            default_bin.to_string()
        } else {
            eprintln!("Usage: {} <path-to-executable.nro | path-to-flat-binary.bin>", args[0]);
            return;
        }
    };

    println!("Examining target binary: {}", rom_path);

    // Read first 0x20 bytes to detect binary format
    let file_bytes = match fs::read(&rom_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to open '{}': {}", rom_path, e);
            std::process::exit(1);
        }
    };

    let is_nro = file_bytes.len() >= 0x20 && &file_bytes[0x10..0x14] == NRO_MAGIC;

    if is_nro {
        run_horizon_nro(&rom_path);
    } else {
        run_baremetal_binary(&rom_path);
    }
}

/// Executes a Nintendo Relocatable Object (NRO) under Horizon OS HLE microkernel.
fn run_horizon_nro(rom_path: &str) {
    println!("Detected format: Nintendo Relocatable Object (NRO0)");
    println!("Starting Horizon OS HLE Subsystem...\n");

    let mut bus = SystemBus::new();
    bus.map_ram(HORIZON_LOAD_BASE, HORIZON_CODE_SIZE);
    bus.map_ram(HORIZON_HEAP_BASE, HORIZON_HEAP_SIZE);

    let header = match load_nro(&mut bus, rom_path, HORIZON_LOAD_BASE) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Failed to load NRO '{}': {}", rom_path, e);
            std::process::exit(1);
        }
    };

    println!("NRO Executable Loaded:");
    println!("  Load Base:   0x{:016X}", HORIZON_LOAD_BASE);
    println!("  Text:        offset: 0x{:08X}, size: 0x{:X}", header.text.file_offset, header.text.size);
    println!("  Rodata:      offset: 0x{:08X}, size: 0x{:X}", header.rodata.file_offset, header.rodata.size);
    println!("  Data:        offset: 0x{:08X}, size: 0x{:X}", header.data.file_offset, header.data.size);
    println!("  BSS Size:    0x{:X} bytes\n", header.bss_size);

    let mut kernel = KernelContext::new(true);
    let profile = kernel.profiles.get_or_default(kernel.process.title_id);
    println!("Game Profile:  {} (Target FPS: {})", profile.title_name, profile.target_fps);
    println!("Root Service:  sm: (Handle: 0x{:04X})", kernel.sm_session_handle);

    let display = emu_core::gpu::DisplaySurface::handheld_720p(0x9000_0000);
    println!("Display Surface: Handheld {}x{} RGBA8_UNORM", display.width, display.height);
    println!("VFS Services:  fsp-srv, RomFS, SaveData (save:/)");
    println!("Audio Services: audout:u, audren:u (DSP 48kHz Stereo)");
    println!("JIT Engine:    BasicBlock Translation & Fastmem Paging");
    println!("Scheduler:     4 Cores (Cores 0-2 Title, Core 3 OS) & Futex Arbitration");
    println!("System HLE:    set:sys, appletOE, acc:u0, hid (200Hz Shared Memory)\n");

    let mut cpu = Cpu::new();
    cpu.reset(HORIZON_LOAD_BASE + header.text.file_offset as u64);

    println!("--- [Horizon Guest Execution Output] ---");

    const MAX_CYCLES: u64 = 1_000_000;
    let mut exit_reason = "Max cycles reached";

    while cpu.cycle_count < MAX_CYCLES {
        match cpu.step(&mut bus) {
            Ok(StepResult::Continue) => continue,
            Ok(StepResult::Svc(svc_id)) => {
                match kernel.handle_svc(svc_id, &mut cpu, &mut bus) {
                    Ok(SvcOutcome::Continue | SvcOutcome::YieldThread) => continue,
                    Ok(SvcOutcome::ExitProcess) => {
                        println!("\n--- [Horizon Process Exited: svcExitProcess] ---");
                        exit_reason = "Process terminated gracefully (svcExitProcess)";
                        break;
                    }
                    Err(e) => {
                        eprintln!("\n--- [Horizon Kernel Error: {}] ---", e);
                        exit_reason = "Kernel Exception";
                        break;
                    }
                }
            }
            Ok(StepResult::Halt) => {
                println!("\n--- [CPU Halted] ---");
                exit_reason = "Halted";
                break;
            }
            Ok(StepResult::Breakpoint) => {
                println!("\n--- [Breakpoint Triggered] ---");
                exit_reason = "Breakpoint";
                break;
            }
            Err(e) => {
                eprintln!("\n--- [CPU Error: {}] ---", e);
                exit_reason = "Illegal CPU Opcode / Fault";
                break;
            }
        }
    }

    println!("\n--- Process Execution Summary ---");
    println!("  Exit Status:  {}", exit_reason);
    println!("  Final PC:     0x{:016X}", cpu.regs.pc);
    println!("  Cycle Count:  {}", cpu.cycle_count);
    println!("  Alloc Heap:   0x{:X} bytes at 0x{:08X}", kernel.process.heap_size, kernel.process.heap_base);
    println!("  Open Handles: {}", kernel.handles.len());
    println!("  Active Threads: {}", kernel.scheduler.active_thread_count());
    println!("  GPU Display:  {}x{} Handheld Surface", display.width, display.height);
}

/// Executes a bare-metal flat ARM64 binary with MMIO UART.
fn run_baremetal_binary(rom_path: &str) {
    println!("Detected format: Raw ARM64 Flat Binary");
    println!("Starting Bare-Metal Interconnect...\n");

    let mut bus = SystemBus::new();
    bus.map_ram(BAREMETAL_RAM_BASE, BAREMETAL_RAM_SIZE);
    bus.map_mmio(UART_BASE, UART_SIZE, Box::new(DebugUart::new(true)));

    let entry_point = match load_flat_binary(&mut bus, rom_path, BAREMETAL_RAM_BASE) {
        Ok(entry) => entry,
        Err(e) => {
            eprintln!("Failed to load bare-metal ROM '{}': {}", rom_path, e);
            std::process::exit(1);
        }
    };

    println!("Mapped RAM:   0x{:08X} - 0x{:08X}", BAREMETAL_RAM_BASE, BAREMETAL_RAM_BASE + BAREMETAL_RAM_SIZE as u64);
    println!("Mapped UART:  0xD0000000 - 0xD0001000");
    println!("Entry Point:  0x{:016X}\n", entry_point);
    println!("--- [Guest UART Output] ---");

    let mut cpu = Cpu::new();
    cpu.reset(entry_point);

    const MAX_CYCLES: u64 = 500_000;
    let mut exit_reason = "Max cycles reached";

    while cpu.cycle_count < MAX_CYCLES {
        match cpu.step(&mut bus) {
            Ok(StepResult::Continue) => continue,
            Ok(StepResult::Svc(imm)) => {
                println!("\n--- [SVC 0x{:04X} Hit] ---", imm);
                exit_reason = "SVC Hit";
                break;
            }
            Ok(StepResult::Halt) => {
                println!("\n--- [Halted] ---");
                exit_reason = "Halted cleanly";
                break;
            }
            Ok(StepResult::Breakpoint) => {
                println!("\n--- [Breakpoint] ---");
                exit_reason = "Breakpoint hit";
                break;
            }
            Err(e) => {
                eprintln!("\n--- [CPU Error: {}] ---", e);
                exit_reason = "CPU Error";
                break;
            }
        }
    }

    println!("\n--- Summary ---");
    println!("  Status:       {}", exit_reason);
    println!("  Final PC:     0x{:016X}", cpu.regs.pc);
    println!("  Cycles:       {}", cpu.cycle_count);
}
