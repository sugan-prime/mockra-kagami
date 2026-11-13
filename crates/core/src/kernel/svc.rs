//! Horizon OS Microkernel Supervisor Call (SVC) dispatch engine.

use crate::cpu::cpu::Cpu;
use crate::kernel::handle_table::HandleTable;
use crate::kernel::ipc::{CmifError, CmifRequest, IpcContext, IpcError, ServiceManager};
use crate::kernel::object::KernelObject;
use crate::kernel::patch::PatchManager;
use crate::kernel::profile::ProfileRegistry;
use crate::memory::bus::{MemoryBus, MemoryError};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum KernelError {
    #[error("Memory access violation: {0}")]
    Memory(#[from] MemoryError),
    #[error("Unhandled Supervisor Call (SVC): 0x{0:02X}")]
    UnhandledSvc(u16),
    #[error("Invalid handle: 0x{0:08X}")]
    InvalidHandle(u32),
    #[error("IPC error: {0}")]
    Ipc(#[from] IpcError),
    #[error("CMIF protocol error: {0}")]
    Cmif(#[from] CmifError),
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
    pub handles: HandleTable,
    pub profiles: ProfileRegistry,
    pub service_manager: Arc<ServiceManager>,
    pub sm_session_handle: u32,
    pub debug_log: Arc<Mutex<String>>,
    pub mirror_stdout: bool,
    pub tls_base: u64,
}

impl Default for KernelContext {
    fn default() -> Self {
        Self::new(true)
    }
}

impl KernelContext {
    pub fn new(mirror_stdout: bool) -> Self {
        let mut handles = HandleTable::new();
        let service_manager = Arc::new(ServiceManager::new());
        let sm_session_handle = handles.create_session("sm:", service_manager.clone());

        Self {
            process: Process::default(),
            patch_manager: PatchManager::new(),
            handles,
            profiles: ProfileRegistry::with_known_titles(),
            service_manager,
            sm_session_handle,
            debug_log: Arc::new(Mutex::new(String::new())),
            mirror_stdout,
            tls_base: 0x1000,
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
                cpu.regs.set_x(0, 0); // ResultCode: Success (0)
                Ok(SvcOutcome::YieldThread)
            }

            // 0x18: svcWaitSynchronization(out_index: &mut i32, handles_ptr: u64, num_handles: i32, timeout_ns: i64)
            0x18 => {
                let handles_arg = cpu.regs.get_x(1);
                let num_handles = cpu.regs.get_w(2) as usize;

                let handle_list = if handles_arg >= 0x1000 && num_handles > 0 {
                    let mut list = Vec::with_capacity(num_handles);
                    for i in 0..num_handles {
                        let h = bus.read_u32(handles_arg.wrapping_add((i * 4) as u64))?;
                        list.push(h);
                    }
                    list
                } else {
                    vec![handles_arg as u32]
                };

                let mut signaled_index = None;
                for (idx, &handle) in handle_list.iter().enumerate() {
                    if let Some(obj) = self.handles.get(handle) {
                        if obj.try_consume_signal() {
                            signaled_index = Some(idx);
                            break;
                        }
                    }
                }

                if let Some(idx) = signaled_index {
                    cpu.regs.set_x(1, idx as u64); // Signaled object index
                    cpu.regs.set_x(0, 0);          // Success
                } else {
                    cpu.regs.set_x(0, 0xEA01);     // Horizon OS ResultTimeout
                }

                Ok(SvcOutcome::Continue)
            }

            // 0x1E: svcSignalEvent(event_handle: u32)
            0x1E => {
                let handle = cpu.regs.get_w(0);
                match self.handles.get(handle) {
                    Some(KernelObject::WritableEvent(event)) => {
                        event.signal();
                        cpu.regs.set_x(0, 0); // Success
                    }
                    _ => {
                        cpu.regs.set_x(0, 0xE401); // Invalid Handle
                    }
                }
                Ok(SvcOutcome::Continue)
            }

            // 0x1F: svcClearEvent(event_handle: u32)
            0x1F => {
                let handle = cpu.regs.get_w(0);
                match self.handles.get(handle) {
                    Some(KernelObject::WritableEvent(event))
                    | Some(KernelObject::ReadableEvent(event)) => {
                        event.clear();
                        cpu.regs.set_x(0, 0); // Success
                    }
                    _ => {
                        cpu.regs.set_x(0, 0xE401); // Invalid Handle
                    }
                }
                Ok(SvcOutcome::Continue)
            }

            // 0x21: svcSendSyncRequest(session_handle: u32)
            0x21 => {
                let session_handle = cpu.regs.get_w(0);
                let tls_addr = if cpu.regs.tpidr_el0 != 0 {
                    cpu.regs.tpidr_el0
                } else {
                    self.tls_base
                };

                // Read 0x100 bytes from TLS command buffer
                let mut tls_buf = [0u8; 0x100];
                for (i, slot) in tls_buf.iter_mut().enumerate() {
                    *slot = bus.read_u8(tls_addr.wrapping_add(i as u64))?;
                }

                let req = CmifRequest::parse(&tls_buf)?;
                let session_opt = match self.handles.get(session_handle) {
                    Some(KernelObject::Session(sess)) => Some(sess.clone()),
                    _ => None,
                };

                if let Some(session) = session_opt {
                    let mut ipc_ctx = IpcContext::new(req);
                    session.handler.dispatch(&mut ipc_ctx)?;

                    // Register newly spawned sessions into handle table
                    for (name, s_handler) in ipc_ctx.spawned_sessions {
                        let new_h = self.handles.create_session(name, s_handler);
                        ipc_ctx.response.push_handle(new_h);
                        ipc_ctx.response.push_u32(new_h);
                    }

                    // Register newly spawned events into handle table
                    for event in ipc_ctx.spawned_events {
                        let new_h = self.handles.insert(KernelObject::ReadableEvent(event));
                        ipc_ctx.response.push_handle(new_h);
                        ipc_ctx.response.push_u32(new_h);
                    }

                    // Write response back to TLS buffer
                    let mut out_buf = [0u8; 0x100];
                    let written = ipc_ctx.response.serialize(&mut out_buf)?;
                    for (i, &b) in out_buf[..written].iter().enumerate() {
                        bus.write_u8(tls_addr.wrapping_add(i as u64), b)?;
                    }

                    cpu.regs.set_x(0, 0); // Success
                } else {
                    cpu.regs.set_x(0, 0xE401); // Invalid Handle
                }

                Ok(SvcOutcome::Continue)
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

            // 0x35: svcSetHeapSize(out_addr: &mut u64, size: u64)
            0x35 => {
                let requested_size = cpu.regs.get_x(1);
                self.process.heap_size = requested_size;

                // Return allocated base in X1 and success (0) in X0
                cpu.regs.set_x(1, self.process.heap_base);
                cpu.regs.set_x(0, 0); // Success
                Ok(SvcOutcome::Continue)
            }

            // 0x45: svcCreateEvent(out_write_handle: &mut u32, out_read_handle: &mut u32, auto_reset: bool)
            0x45 => {
                let auto_reset = cpu.regs.get_w(1) != 0 || cpu.regs.get_w(0) != 0;
                let (readable, writable) = self.handles.create_event_pair(auto_reset);

                // Return writable in X1 and readable in X2
                cpu.regs.set_x(1, writable as u64);
                cpu.regs.set_x(2, readable as u64);
                cpu.regs.set_x(0, 0); // Success
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

    #[test]
    fn test_svc_event_creation_signaling_and_wait() {
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new(0, 0x10000);
        let mut kernel = KernelContext::new(false);

        // 1. Create auto-reset event (SVC 0x45)
        cpu.regs.set_x(1, 1); // auto_reset = true
        kernel.handle_svc(0x45, &mut cpu, &mut bus).unwrap();
        assert_eq!(cpu.regs.get_x(0), 0);
        let writable_h = cpu.regs.get_w(1);
        let readable_h = cpu.regs.get_w(2);

        // 2. Wait on readable handle before signal -> should timeout
        cpu.regs.set_x(1, readable_h as u64);
        cpu.regs.set_w(2, 1); // 1 handle
        kernel.handle_svc(0x18, &mut cpu, &mut bus).unwrap();
        assert_eq!(cpu.regs.get_w(0), 0xEA01); // ResultTimeout

        // 3. Signal event using writable handle (SVC 0x1E)
        cpu.regs.set_x(0, writable_h as u64);
        kernel.handle_svc(0x1E, &mut cpu, &mut bus).unwrap();
        assert_eq!(cpu.regs.get_x(0), 0);

        // 4. Wait again -> should succeed
        cpu.regs.set_x(1, readable_h as u64);
        cpu.regs.set_w(2, 1);
        kernel.handle_svc(0x18, &mut cpu, &mut bus).unwrap();
        assert_eq!(cpu.regs.get_x(0), 0); // Success!
        assert_eq!(cpu.regs.get_x(1), 0); // Index 0 signaled

        // 5. Subsequent wait should timeout because of auto-reset
        kernel.handle_svc(0x18, &mut cpu, &mut bus).unwrap();
        assert_eq!(cpu.regs.get_w(0), 0xEA01);
    }

    #[test]
    fn test_svc_ipc_sync_request_sm_and_time() {
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new(0, 0x10000);
        let mut kernel = KernelContext::new(false);

        let sm_handle = kernel.sm_session_handle;
        let tls_addr = 0x1000;
        kernel.tls_base = tls_addr;

        // Step A: Request "time" service from ServiceManager (sm:)
        // Write CMIF request into TLS: Magic "SFCI", cmd 1 (GetService), payload "time\0\0\0\0"
        bus.write_u8(tls_addr + 0, b'S').unwrap();
        bus.write_u8(tls_addr + 1, b'F').unwrap();
        bus.write_u8(tls_addr + 2, b'C').unwrap();
        bus.write_u8(tls_addr + 3, b'I').unwrap();
        bus.write_u32(tls_addr + 8, 1).unwrap(); // Cmd 1: GetService
        for (i, &b) in b"time\0\0\0\0".iter().enumerate() {
            bus.write_u8(tls_addr + 16 + i as u64, b).unwrap();
        }

        // Send IPC request to ServiceManager
        cpu.regs.set_x(0, sm_handle as u64);
        kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
        assert_eq!(cpu.regs.get_x(0), 0); // Success

        // Read time service handle from response payload
        let time_handle = bus.read_u32(tls_addr + 16).unwrap();
        assert_ne!(time_handle, 0);

        // Step B: Query "time:" service for current POSIX timestamp (Cmd 0)
        bus.write_u8(tls_addr + 0, b'S').unwrap();
        bus.write_u8(tls_addr + 1, b'F').unwrap();
        bus.write_u8(tls_addr + 2, b'C').unwrap();
        bus.write_u8(tls_addr + 3, b'I').unwrap();
        bus.write_u32(tls_addr + 8, 0).unwrap(); // Cmd 0: GetCurrentTime
        cpu.regs.set_x(0, time_handle as u64);
        kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
        assert_eq!(cpu.regs.get_x(0), 0);

        // Verify timestamp returned in response payload
        let timestamp = bus.read_u64(tls_addr + 16).unwrap();
        assert!(timestamp > 1600000000);
    }
}
