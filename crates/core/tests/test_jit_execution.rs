//! End-to-end integration tests for JIT Compilation, Fastmem, and Basic Block Execution.

use emu_core::cpu::jit::engine::JitEngine;
use emu_core::cpu::{Cpu, StepResult};
use emu_core::kernel::KernelContext;
use emu_core::memory::fastmem::{permissions, Fastmem};
use emu_core::memory::MemoryBus;

#[test]
fn test_jit_fibonacci_compute_loop_fastmem() {
    let mut mem_jit = Fastmem::new();
    let mut mem_interp = Fastmem::new();

    // Map 64KB RAM at 0x8000_0000 with Read/Write/Execute permissions
    mem_jit.map_range(0x8000_0000, 0x10000, permissions::RWX);
    mem_interp.map_range(0x8000_0000, 0x10000, permissions::RWX);

    // ARM64 Fibonacci sequence calculation up to N = 15 (expected fib(15) = 610)
    // 0x8000_0000: MOVZ X0, #0        ; fib(0) = 0
    // 0x8000_0004: MOVZ X1, #1        ; fib(1) = 1
    // 0x8000_0008: MOVZ X2, #14       ; counter = 14 iterations
    // loop (0x8000_000C):
    // 0x8000_000C: ADD X3, X0, X1     ; next = fib0 + fib1
    // 0x8000_0010: ORR X0, XZR, X1    ; fib0 = fib1
    // 0x8000_0014: ORR X1, XZR, X3    ; fib1 = next
    // 0x8000_0018: SUBS X2, X2, #1    ; counter--
    // 0x8000_001C: B.NE loop (-16B)   ; branch if counter != 0
    // 0x8000_0020: SVC #42            ; done
    let program: [u32; 9] = [
        0xD2800000, // movz x0, #0
        0xD2800021, // movz x1, #1
        0xD28001C2, // movz x2, #14
        0x8B010003, // add x3, x0, x1
        0xAA0103E0, // orr x0, xzr, x1
        0xAA0303E1, // orr x1, xzr, x3
        0xF1000442, // subs x2, x2, #1
        0x54FFFF81, // b.ne -16 (target 0x8000_000C)
        0xD4000541, // svc #42
    ];

    for (i, &instr) in program.iter().enumerate() {
        mem_jit.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        mem_interp.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
    }

    // 1. Run via JIT Engine
    let mut cpu_jit = Cpu::new();
    cpu_jit.reset(0x8000_0000);
    let mut jit_engine = JitEngine::new();

    loop {
        match jit_engine.step(&mut cpu_jit, &mut mem_jit).unwrap() {
            StepResult::Continue => continue,
            StepResult::Svc(42) => break,
            other => panic!("Unexpected JIT StepResult: {:?}", other),
        }
    }

    // 2. Run identical binary via pure Interpreter
    let mut cpu_interp = Cpu::new();
    cpu_interp.reset(0x8000_0000);

    loop {
        match cpu_interp.step(&mut mem_interp).unwrap() {
            StepResult::Continue => continue,
            StepResult::Svc(42) => break,
            other => panic!("Unexpected Interp StepResult: {:?}", other),
        }
    }

    // Verify both execution engines arrive at the exact same architectural state!
    assert_eq!(cpu_jit.regs.get_x(1), 610, "fib(15) must be 610 in JIT");
    assert_eq!(cpu_interp.regs.get_x(1), 610, "fib(15) must be 610 in Interpreter");
    assert_eq!(cpu_jit.regs.get_x(1), cpu_interp.regs.get_x(1));
    assert_eq!(cpu_jit.regs.get_x(2), 0);
    assert_eq!(cpu_interp.regs.get_x(2), 0);

    // Verify JIT metrics: high block reuse, 0 interpreter fallbacks
    assert!(jit_engine.metrics.blocks_executed > 10);
    assert_eq!(jit_engine.metrics.fallback_instructions, 0);
    assert!(jit_engine.cache.stats.hits > 10);
}

#[test]
fn test_jit_memory_load_store_pair_fastmem() {
    let mut mem = Fastmem::new();
    mem.map_range(0x8000_0000, 0x10000, permissions::RWX);

    // Program:
    // 0x8000_0000: MOVZ X0, #0x1234
    // 0x8000_0004: MOVZ X1, #0x5678
    // 0x8000_0008: MOVZ X2, #0x2000 (offset = 0x8000_2000)
    // 0x8000_000C: MOVK X2, #0x8000, lsl 16
    // 0x8000_0010: STP X0, X1, [X2, #0]
    // 0x8000_0014: LDP X3, X4, [X2, #0]
    // 0x8000_0018: SVC #10
    let program: [u32; 7] = [
        0xD2824680, // movz x0, #0x1234
        0xD28ACF01, // movz x1, #0x5678
        0xD2840002, // movz x2, #0x2000
        0xF2B00002, // movk x2, #0x8000, lsl #16 -> x2 = 0x80002000
        0xA9000440, // stp x0, x1, [x2, #0]
        0xA9401043, // ldp x3, x4, [x2, #0]
        0xD4000141, // svc #10
    ];

    for (i, &instr) in program.iter().enumerate() {
        mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
    }

    let mut cpu = Cpu::new();
    cpu.reset(0x8000_0000);
    let mut jit = JitEngine::new();

    loop {
        match jit.step(&mut cpu, &mut mem).unwrap() {
            StepResult::Continue => continue,
            StepResult::Svc(10) => break,
            other => panic!("Unexpected StepResult: {:?}", other),
        }
    }

    assert_eq!(cpu.regs.get_x(3), 0x1234);
    assert_eq!(cpu.regs.get_x(4), 0x5678);
}

#[test]
fn test_jit_interop_with_horizon_svcs() {
    let mut mem = Fastmem::new();
    mem.map_range(0x8000_0000, 0x10000, permissions::RWX);

    let mut cpu = Cpu::new();
    cpu.reset(0x8000_0000);
    let mut kernel = KernelContext::new(false);

    // Setup string at 0x8000_0100: "JIT Fastmem Output\n"
    let msg = b"JIT Fastmem Output\n";
    for (i, &b) in msg.iter().enumerate() {
        mem.write_u8(0x8000_0100 + i as u64, b).unwrap();
    }

    // Program:
    // 0x8000_0000: MOVZ X0, #0x0100
    // 0x8000_0004: MOVK X0, #0x8000, lsl 16 (ptr = 0x80000100)
    // 0x8000_0008: MOVZ X1, #19 (length = 19)
    // 0x8000_000C: SVC #0x27 (svcOutputDebugString)
    // 0x8000_0010: SVC #0x07 (svcExitProcess)
    let program: [u32; 5] = [
        0xD2802000, // movz x0, #0x0100
        0xF2B00000, // movk x0, #0x8000, lsl #16
        0xD2800261, // movz x1, #19
        0xD40004E1, // svc #0x27 (output debug string)
        0xD40000E1, // svc #0x07 (exit process)
    ];

    for (i, &instr) in program.iter().enumerate() {
        mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
    }

    let mut jit = JitEngine::new();
    let mut exited = false;

    for _ in 0..100 {
        match jit.step(&mut cpu, &mut mem).unwrap() {
            StepResult::Continue => continue,
            StepResult::Svc(imm) => {
                let outcome = kernel.handle_svc(imm, &mut cpu, &mut mem).unwrap();
                if let emu_core::kernel::SvcOutcome::ExitProcess = outcome {
                    exited = true;
                    break;
                }
            }
            _ => break,
        }
    }

    assert!(exited, "Process should exit cleanly via SVC 0x07");
    let log = kernel.debug_log.lock().unwrap().clone();
    assert!(log.contains("JIT Fastmem Output"));
}
