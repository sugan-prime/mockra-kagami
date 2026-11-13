//! End-to-end integration tests for Horizon OS Multi-Core Threading,
//! Scheduler, Context Switching, Event Synchronization, and Futex Arbitration.

use emu_core::cpu::Cpu;
use emu_core::kernel::{KernelContext, SvcOutcome, ThreadStatus};
use emu_core::memory::fastmem::{permissions, Fastmem};

#[test]
fn test_multithread_worker_event_signaling() {
    let mut mem = Fastmem::new();
    mem.map_range(0x8000_0000, 0x20000, permissions::RWX);

    let mut cpu_main = Cpu::new();
    cpu_main.reset(0x8000_0000);
    let mut kernel = KernelContext::new(false);

    // 1. Create event pair: readable and writable handles
    let (read_handle, write_handle) = kernel.handles.create_event_pair(true);

    // 2. Main thread creates worker thread (SVC 0x08)
    // Worker Entry: 0x8000_1000, Arg (X0): write_handle, Stack: 0x8000_E000, Priority: 44, Core: 1
    cpu_main.regs.set_x(1, 0x8000_1000); // entry
    cpu_main.regs.set_x(2, write_handle as u64); // arg in X0
    cpu_main.regs.set_x(3, 0x8000_E000); // stack top
    cpu_main.regs.set_w(4, 44);          // priority
    cpu_main.regs.set_w(5, 1);           // core 1

    kernel.handle_svc(0x08, &mut cpu_main, &mut mem).unwrap();
    assert_eq!(cpu_main.regs.get_x(0), 0); // ResultSuccess
    let thread_handle = cpu_main.regs.get_x(1) as u32;
    assert!(thread_handle > 0);

    // 3. Main thread starts worker thread (SVC 0x09)
    cpu_main.regs.set_w(0, thread_handle);
    kernel.handle_svc(0x09, &mut cpu_main, &mut mem).unwrap();
    assert_eq!(cpu_main.regs.get_x(0), 0);
    assert_eq!(kernel.scheduler.run_queues[1].len(), 1);

    // 4. Main thread waits on readable event (SVC 0x18)
    cpu_main.regs.set_x(1, read_handle as u64);
    cpu_main.regs.set_w(2, 1); // 1 handle
    kernel.handle_svc(0x18, &mut cpu_main, &mut mem).unwrap();
    // Initially unsignaled -> Timeout (0xEA01)
    assert_eq!(cpu_main.regs.get_w(0), 0xEA01);

    // 5. Worker thread execution on Core 1
    let mut cpu_worker = Cpu::new();
    let worker_tid = kernel.scheduler.context_switch(1, &mut cpu_worker.regs).unwrap();
    assert_eq!(worker_tid, 2);
    assert_eq!(cpu_worker.regs.pc, 0x8000_1000);
    assert_eq!(cpu_worker.regs.get_x(0), write_handle as u64);

    // Worker signals the event (SVC 0x1E)
    // Arg X0 is already write_handle
    kernel.handle_svc(0x1E, &mut cpu_worker, &mut mem).unwrap();
    assert_eq!(cpu_worker.regs.get_x(0), 0);

    // Worker exits (SVC 0x0A)
    let outcome = kernel.handle_svc(0x0A, &mut cpu_worker, &mut mem).unwrap();
    assert_eq!(outcome, SvcOutcome::YieldThread);
    assert_eq!(kernel.scheduler.threads.get(&worker_tid).unwrap().status, ThreadStatus::Terminated);

    // 6. Main thread waits on readable event again -> now signaled!
    cpu_main.regs.set_x(1, read_handle as u64);
    cpu_main.regs.set_w(2, 1);
    kernel.handle_svc(0x18, &mut cpu_main, &mut mem).unwrap();
    assert_eq!(cpu_main.regs.get_x(0), 0); // Success!
    assert_eq!(cpu_main.regs.get_x(1), 0); // Index 0 signaled
}

#[test]
fn test_multithread_futex_lock_and_unlock() {
    let mut mem = Fastmem::new();
    mem.map_range(0x8000_0000, 0x10000, permissions::RWX);

    let mut cpu = Cpu::new();
    cpu.reset(0x8000_0000);
    let mut kernel = KernelContext::new(false);

    const MUTEX_ADDR: u64 = 0x8000_4000;

    // Main thread acquires and locks futex (SVC 0x1A)
    cpu.regs.set_x(1, MUTEX_ADDR);
    let outcome = kernel.handle_svc(0x1A, &mut cpu, &mut mem).unwrap();
    assert_eq!(outcome, SvcOutcome::YieldThread);

    assert_eq!(kernel.scheduler.futex_waiters.get(&MUTEX_ADDR).unwrap().len(), 1);

    // Unlocking futex (SVC 0x1B) awakens waiting thread
    cpu.regs.set_x(0, MUTEX_ADDR);
    kernel.handle_svc(0x1B, &mut cpu, &mut mem).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    assert!(kernel.scheduler.futex_waiters.get(&MUTEX_ADDR).unwrap().is_empty());
}
