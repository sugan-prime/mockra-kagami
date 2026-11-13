use emu_core::cpu::{Cpu, StepResult};
use emu_core::loader::load_flat_binary;
use emu_core::memory::{DebugUart, SystemBus};
use std::env;
use std::path::Path;

const RAM_BASE: u64 = 0x8000_0000;
const RAM_SIZE: usize = 0x100000; // 1 MB
const UART_BASE: u64 = 0xD000_0000;
const UART_SIZE: u64 = 0x1000;

fn main() {
    println!("=== Switch-Emulator (Rust AArch64 Bare-Metal Runner) ===\n");

    let args: Vec<String> = env::args().collect();
    let rom_path = if args.len() > 1 {
        args[1].clone()
    } else {
        // Fallback to built-in test ROM if available
        let default_rom = "tests/test_roms/hello_uart.bin";
        if Path::new(default_rom).exists() {
            println!("No ROM path provided. Running default test ROM: {}", default_rom);
            default_rom.to_string()
        } else {
            eprintln!("Usage: {} <path-to-flat-binary.bin>", args[0]);
            return;
        }
    };

    println!("Loading ROM: {}", rom_path);

    // Initialize System Bus
    let mut bus = SystemBus::new();
    bus.map_ram(RAM_BASE, RAM_SIZE);

    // Attach Debug UART peripheral mapped at 0xD000_0000 (mirroring to stdout)
    bus.map_mmio(UART_BASE, UART_SIZE, Box::new(DebugUart::new(true)));

    // Load binary into memory
    let entry_point = match load_flat_binary(&mut bus, &rom_path, RAM_BASE) {
        Ok(entry) => entry,
        Err(e) => {
            eprintln!("Failed to load ROM '{}': {}", rom_path, e);
            std::process::exit(1);
        }
    };

    println!("Mapped RAM:   0x{:08X} - 0x{:08X}", RAM_BASE, RAM_BASE + RAM_SIZE as u64);
    println!("Mapped UART:  0x{:08X} - 0x{:08X}", UART_BASE, UART_BASE + UART_SIZE);
    println!("Entry Point:  0x{:016X}\n", entry_point);
    println!("--- [Guest Execution Output] ---");

    let mut cpu = Cpu::new();
    cpu.reset(entry_point);

    const MAX_CYCLES: u64 = 500_000;
    let mut exit_reason = "Max cycles reached";

    while cpu.cycle_count < MAX_CYCLES {
        match cpu.step(&mut bus) {
            Ok(StepResult::Continue) => continue,
            Ok(StepResult::Svc(imm)) => {
                println!("\n--- [Execution Ended: SVC 0x{:04X}] ---", imm);
                exit_reason = "Supervisor Call (SVC)";
                break;
            }
            Ok(StepResult::Halt) => {
                println!("\n--- [Execution Ended: HALT] ---");
                exit_reason = "Halted cleanly";
                break;
            }
            Ok(StepResult::Breakpoint) => {
                println!("\n--- [Execution Ended: BREAKPOINT] ---");
                exit_reason = "Breakpoint hit";
                break;
            }
            Err(e) => {
                eprintln!("\n--- [Execution Aborted: CPU Error: {}] ---", e);
                exit_reason = "CPU Error";
                break;
            }
        }
    }

    println!("\n--- Summary ---");
    println!("  Status:       {}", exit_reason);
    println!("  PC:           0x{:016X}", cpu.regs.pc);
    println!("  SP:           0x{:016X}", cpu.regs.sp);
    println!("  Total Cycles: {}", cpu.cycle_count);
    println!("  Flags (NZCV): N={} Z={} C={} V={}",
        cpu.regs.pstate.n as u8,
        cpu.regs.pstate.z as u8,
        cpu.regs.pstate.c as u8,
        cpu.regs.pstate.v as u8
    );
}
