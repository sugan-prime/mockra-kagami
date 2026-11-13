use emu_core::cpu::{Cpu, StepResult};
use emu_core::memory::{FlatMemory, MemoryBus};

fn main() {
    println!("=== Switch-Emulator (Rust AArch64 Core Prototype) ===");
    println!("Milestone 1: Testing Memory Access, Flags & Control Flow\n");

    // Initialize 64KB memory mapped at 0x8000_0000
    const BASE_ADDR: u64 = 0x8000_0000;
    const DATA_ADDR: u64 = 0x8000_2000;
    let mut mem = FlatMemory::new(BASE_ADDR, 0x10000);

    // Bare-metal test program:
    // 1. MOVZ X0, #10           ; X0 = 10 (loop counter)
    // 2. MOVZ X1, #0            ; X1 = 0  (accumulator)
    // 3. MOVZ X2, #0x8000, LSL 16 ; Base address setup: 0x8000_2000
    // 4. MOVK X2, #0x2000
    // loop:
    // 5. ADD X1, X1, X0         ; X1 += X0
    // 6. SUBS X0, X0, #1        ; X0 -= 1, update flags
    // 7. B.NE loop              ; if X0 != 0 (Z == 0), repeat
    // 8. STR X1, [X2, #0]       ; Store final sum (55) into memory at 0x8000_2000
    // 9. LDR X3, [X2, #0]       ; Read it back from memory into X3
    // 10. SVC #0x42             ; Horizon OS SVC exit trap
    let program: [u32; 10] = [
        0xD2800140, // movz x0, #10
        0xD2800001, // movz x1, #0
        0xD2B00002, // movz x2, #0x8000, lsl #16
        0xF2840002, // movk x2, #0x2000
        0x8B000021, // add x1, x1, x0
        0xF1000400, // subs x0, x0, #1
        0x54FFFFC1, // b.ne -2 (back to add x1, x1, x0)
        0xF9000041, // str x1, [x2, #0]
        0xF9400043, // ldr x3, [x2, #0]
        0xD4000841, // svc #0x42
    ];

    for (i, &word) in program.iter().enumerate() {
        mem.load(BASE_ADDR + (i as u64) * 4, &word.to_le_bytes())
            .expect("Failed to load test instruction");
    }

    let mut cpu = Cpu::new();
    cpu.reset(BASE_ADDR);

    println!("Starting execution at PC: 0x{:016X}...\n", cpu.regs.pc);

    let mut steps = 0;
    loop {
        steps += 1;
        let pc_before = cpu.regs.pc;
        match cpu.step(&mut mem) {
            Ok(StepResult::Continue) => {
                // To avoid flooding stdout during loops, print key steps
                if steps <= 4 || cpu.regs.pc > BASE_ADDR + 24 {
                    println!("[Step {:04}] Executed at 0x{:08X} (PC now: 0x{:08X})", cpu.cycle_count, pc_before, cpu.regs.pc);
                }
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
    println!("  X0 (Counter):     {} (0x{:016X})", cpu.regs.get_x(0), cpu.regs.get_x(0));
    println!("  X1 (Sum Result):  {} (0x{:016X})", cpu.regs.get_x(1), cpu.regs.get_x(1));
    println!("  X2 (Target Addr): 0x{:016X}", cpu.regs.get_x(2));
    println!("  X3 (Loaded Back): {} (0x{:016X})", cpu.regs.get_x(3), cpu.regs.get_x(3));
    println!("  SP:               0x{:016X}", cpu.regs.sp);
    println!("  PC:               0x{:016X}", cpu.regs.pc);
    println!("  Flags (NZCV):     N={} Z={} C={} V={}", 
        cpu.regs.pstate.n as u8, 
        cpu.regs.pstate.z as u8, 
        cpu.regs.pstate.c as u8, 
        cpu.regs.pstate.v as u8
    );
    println!("  Total Cycles:     {}", cpu.cycle_count);

    let mem_val = mem.read_u64(DATA_ADDR).unwrap();
    println!("\n--- Memory Verification ---");
    println!("  Value at 0x{:08X}: {} (0x{:016X})", DATA_ADDR, mem_val, mem_val);
    assert_eq!(mem_val, 55, "Memory verification failed!");
    assert_eq!(cpu.regs.get_x(3), 55, "LDR verification failed!");
    println!("  -> Memory write and read-back successfully verified!");
}
