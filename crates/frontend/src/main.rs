use emu_core::cpu::{Cpu, StepResult};
use emu_core::memory::FlatMemory;

fn main() {
    println!("=== Switch-Emulator (Rust AArch64 Core Prototype) ===");

    // Initialize 64KB memory mapped at 0x8000_0000
    const BASE_ADDR: u64 = 0x8000_0000;
    let mut mem = FlatMemory::new(BASE_ADDR, 0x10000);

    // Bare-metal test program:
    // 1. MOVZ X0, #100
    // 2. MOVZ X1, #250
    // 3. ADD X2, X0, X1  (X2 = 350)
    // 4. SVC #0x42       (Trigger Horizon OS SVC exit / trap)
    let program: [u32; 4] = [
        0xD2800C80, // movz x0, #100
        0xD2801F41, // movz x1, #250
        0x8B010002, // add x2, x0, x1
        0xD4000841, // svc #0x42
    ];

    for (i, &word) in program.iter().enumerate() {
        mem.load(BASE_ADDR + (i as u64) * 4, &word.to_le_bytes())
            .expect("Failed to load test instruction");
    }

    let mut cpu = Cpu::new();
    cpu.reset(BASE_ADDR);

    println!("Starting execution at PC: 0x{:016X}...\n", cpu.regs.pc);

    loop {
        let pc_before = cpu.regs.pc;
        match cpu.step(&mut mem) {
            Ok(StepResult::Continue) => {
                println!("[Step {:04}] Executed at 0x{:08X}", cpu.cycle_count, pc_before);
            }
            Ok(StepResult::Svc(imm)) => {
                println!("\n[TRAP] Supervisor Call (SVC 0x{:04X}) invoked at PC: 0x{:08X}", imm, pc_before);
                break;
            }
            Ok(StepResult::Halt) => {
                println!("\n[HALT] CPU halted cleanly.");
                break;
            }
            Ok(StepResult::Breakpoint) => {
                println!("\n[BREAK] Breakpoint encountered at 0x{:08X}", pc_before);
                break;
            }
            Err(e) => {
                eprintln!("\n[CPU ERROR] {}", e);
                break;
            }
        }
    }

    println!("\n--- Register Dump ---");
    for i in 0..4 {
        println!("  X{}: 0x{:016X} ({})", i, cpu.regs.get_x(i), cpu.regs.get_x(i));
    }
    println!("  SP: 0x{:016X}", cpu.regs.sp);
    println!("  PC: 0x{:016X}", cpu.regs.pc);
    println!("  Cycles: {}", cpu.cycle_count);
}
