//! Horizon OS Microkernel Supervisor Call (SVC) dispatch engine.

use crate::cpu::cpu::Cpu;
use crate::kernel::patch::PatchManager;
use crate::memory::bus::{MemoryBus, MemoryError};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum KernelError {
    #[error("Memory access violation: {0}")]
    Memory(#[from] MemoryError),
    #[error("Unhandled Supervisor Call (SVC): 0x{0:02X}")]
    UnhandledSvc(u16),
}

/// Execution outcome of an SVC handling step.
#[derive(Debug, PartialEq, Eq)]
pub enum SvcOutcome {
    Continue,
    ExitProcess,
    YieldThread,
}

/// Process control block containing process memory state.
#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u64,
    pub title_id: u64,
    pub heap_base: u64,
    pub heap_size: u64,
}

impl Default for Process {
    fn default() -> Self {
        Self {
            pid: 1,
            title_id: 0x0100000000000000,
            heap_base: 0x8800_0000, // Standard homebrew default heap region
            heap_size: 0,
        }
    }
}

/// Horizon OS High-Level Emulation (HLE) Microkernel context.
pub struct KernelContext {
    pub process: Process,
    pub patch_manager: PatchManager,
    pub debug_log: Arc<Mutex<String>>,
    pub mirror_stdout: bool,
}

impl Default for KernelContext {
    fn default() -> Self {
        Self::new(true)
    }
}

impl KernelContext {
    pub fn new(mirror_stdout: bool) -> Self {
        Self {
            process: Process::default(),
            patch_manager: PatchManager::new(),
            debug_log: Arc::new(Mutex::new(String::new())),
            mirror_stdout,
        }
    }

    /// Returns all strings output via `svcOutputDebugString`.
    pub fn get_debug_log(&self) -> String {
        self.debug_log.lock().unwrap().clone()
    }

    /// Dispatches an SVC invocation using the AArch64 ABI convention.
    pub fn handle_svc(
        &mut self,
        svc_id: u16,
        cpu: &mut Cpu,
        bus: &mut impl MemoryBus,
    ) -> Result<SvcOutcome, KernelError> {
        match svc_id {
            // 0x07: svcExitProcess()
            0x07 => {
                cpu.is_halted = true;
                Ok(SvcOutcome::ExitProcess)
            }

            // 0x0B: svcSleepThread(nanoseconds: u64)
            0x0B => {
                // In a single-threaded interpreter, sleep is a no-op / yield
                cpu.regs.set_x(0, 0); // ResultCode: Success (0)
                Ok(SvcOutcome::YieldThread)
            }

            // 0x27: svcOutputDebugString(str_ptr: u64, length: u64)
            0x27 => {
                let str_ptr = cpu.regs.get_x(0);
                let length = cpu.regs.get_x(1) as usize;

                let mut bytes = Vec::with_capacity(length);
                for i in 0..length {
                    let byte = bus.read_u8(str_ptr.wrapping_add(i as u64))?;
                    bytes.push(byte);
                }

                let text = String::from_utf8_lossy(&bytes).to_string();
                if self.mirror_stdout {
                    print!("{}", text);
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }

                self.debug_log.lock().unwrap().push_str(&text);
                cpu.regs.set_x(0, 0); // Success
                Ok(SvcOutcome::Continue)
            }

            // 0x35: svcSetHeapSize(out_addr: &mut u64, size: u64)
            0x35 => {
                let requested_size = cpu.regs.get_x(1);
                self.process.heap_size = requested_size;

                // Return allocated base in X1 and success (0) in X0
                cpu.regs.set_x(1, self.process.heap_base);
                cpu.regs.set_x(0, 0); // Success
                Ok(SvcOutcome::Continue)
            }

            // 0x29: svcGetInfo(out: &mut u64, id: u32, handle: u32, sub_id: u64)
            0x29 => {
                let info_id = cpu.regs.get_w(1);
                let value = match info_id {
                    0 => 0xF,                // AllowedCpuCoreMask (cores 0..3)
                    2 => 0x1_0000_0000,      // TotalPhysicalMemoryAvailable (4 GB)
                    3 => 0x0001_0000,        // UsedPhysicalMemory
                    12 => 0x1_0000_0000,     // TotalMemoryAvailable
                    _ => 0,
                };

                cpu.regs.set_x(1, value); // Out value in X1
                cpu.regs.set_x(0, 0);     // ResultCode in X0 = Success
                Ok(SvcOutcome::Continue)
            }

            _ => Err(KernelError::UnhandledSvc(svc_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_svc_output_debug_string() {
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new(0, 0x10000);
        let mut kernel = KernelContext::new(false);

        let msg = "Hello Horizon!\n";
        let str_addr = 0x2000;
        for (i, b) in msg.bytes().enumerate() {
            bus.write_u8(str_addr + i as u64, b).unwrap();
        }

        cpu.regs.set_x(0, str_addr);
        cpu.regs.set_x(1, msg.len() as u64);

        let outcome = kernel.handle_svc(0x27, &mut cpu, &mut bus).unwrap();
        assert_eq!(outcome, SvcOutcome::Continue);
        assert_eq!(cpu.regs.get_x(0), 0); // Success
        assert_eq!(kernel.get_debug_log(), msg);
    }

    #[test]
    fn test_svc_set_heap_size() {
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new(0, 0x10000);
        let mut kernel = KernelContext::new(false);

        cpu.regs.set_x(1, 0x20_0000); // 2 MB
        let outcome = kernel.handle_svc(0x35, &mut cpu, &mut bus).unwrap();
        assert_eq!(outcome, SvcOutcome::Continue);
        assert_eq!(cpu.regs.get_x(0), 0); // Success
        assert_eq!(cpu.regs.get_x(1), 0x8800_0000); // Heap base
        assert_eq!(kernel.process.heap_size, 0x20_0000);
    }

    #[test]
    fn test_svc_exit_process() {
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new(0, 0x10000);
        let mut kernel = KernelContext::new(false);

        let outcome = kernel.handle_svc(0x07, &mut cpu, &mut bus).unwrap();
        assert_eq!(outcome, SvcOutcome::ExitProcess);
        assert!(cpu.is_halted);
    }
}
