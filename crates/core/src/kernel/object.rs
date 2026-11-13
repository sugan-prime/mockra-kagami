//! Horizon OS Microkernel Objects.
//!
//! Models core kernel synchronization and IPC objects: Events, Threads, Processes, and Sessions.

use crate::cpu::registers::RegisterFile;
use crate::kernel::ipc::ServiceHandler;
use std::sync::{Arc, Mutex};

pub const CURRENT_PROCESS_PSEUDO_HANDLE: u32 = 0xFFFF_8001;
pub const CURRENT_THREAD_PSEUDO_HANDLE: u32 = 0xFFFF_8000;
pub const INVALID_HANDLE: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    Runnable,
    Waiting,
    Paused,
    Terminated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Created,
    Running,
    Exited,
}

/// Shared internal state for a synchronization event.
#[derive(Debug, Default)]
pub struct EventState {
    pub signaled: Mutex<bool>,
    pub auto_reset: bool,
}

/// Synchronization event primitive.
#[derive(Debug, Clone)]
pub struct KEvent {
    pub state: Arc<EventState>,
}

impl KEvent {
    pub fn new(auto_reset: bool) -> Self {
        Self {
            state: Arc::new(EventState {
                signaled: Mutex::new(false),
                auto_reset,
            }),
        }
    }

    /// Signals the event, unblocking waiting threads.
    pub fn signal(&self) {
        let mut s = self.state.signaled.lock().unwrap();
        *s = true;
    }

    /// Resets the event to unsignaled state.
    pub fn clear(&self) {
        let mut s = self.state.signaled.lock().unwrap();
        *s = false;
    }

    /// Checks if the event is currently signaled.
    pub fn is_signaled(&self) -> bool {
        *self.state.signaled.lock().unwrap()
    }

    /// Checks if signaled; if auto-reset is enabled, resets state to unsignaled.
    pub fn try_consume_signal(&self) -> bool {
        let mut s = self.state.signaled.lock().unwrap();
        if *s {
            if self.state.auto_reset {
                *s = false;
            }
            true
        } else {
            false
        }
    }
}

/// A kernel thread descriptor.
#[derive(Debug, Clone)]
pub struct KThread {
    pub thread_id: u64,
    pub name: String,
    pub entry_point: u64,
    pub stack_top: u64,
    pub priority: i32,
    pub status: ThreadStatus,
    pub regs: RegisterFile,
    pub core_id: u8,
    pub arg: u64,
    pub wait_handle: Option<u32>,
}

impl KThread {
    pub fn new(
        thread_id: u64,
        name: impl Into<String>,
        entry_point: u64,
        stack_top: u64,
        priority: i32,
    ) -> Self {
        let mut regs = RegisterFile::default();
        regs.pc = entry_point;
        regs.sp = stack_top;
        regs.tpidr_el0 = thread_id;
        Self {
            thread_id,
            name: name.into(),
            entry_point,
            stack_top,
            priority,
            status: ThreadStatus::Runnable,
            regs,
            core_id: 0,
            arg: 0,
            wait_handle: None,
        }
    }
}

/// A kernel process descriptor.
#[derive(Debug, Clone)]
pub struct KProcess {
    pub process_id: u64,
    pub title_id: u64,
    pub name: String,
    pub status: ProcessStatus,
    pub main_thread_id: u64,
}

impl KProcess {
    pub fn new(process_id: u64, title_id: u64, name: impl Into<String>) -> Self {
        Self {
            process_id,
            title_id,
            name: name.into(),
            status: ProcessStatus::Running,
            main_thread_id: 1,
        }
    }
}

/// An IPC client/server session endpoint.
#[derive(Clone)]
pub struct KSession {
    pub service_name: String,
    pub handler: Arc<dyn ServiceHandler>,
}

impl std::fmt::Debug for KSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KSession")
            .field("service_name", &self.service_name)
            .finish()
    }
}

/// Unified Horizon Kernel Object.
#[derive(Debug, Clone)]
pub enum KernelObject {
    /// Readable end of an event (can be waited on via `svcWaitSynchronization`).
    ReadableEvent(KEvent),
    /// Writable end of an event (can be signaled via `svcSignalEvent` or cleared via `svcClearEvent`).
    WritableEvent(KEvent),
    /// Thread descriptor.
    Thread(Arc<Mutex<KThread>>),
    /// Process descriptor.
    Process(Arc<Mutex<KProcess>>),
    /// IPC session connected to an HLE service.
    Session(KSession),
}

impl KernelObject {
    /// Returns true if this object can be waited on by `svcWaitSynchronization`.
    pub fn is_waitable(&self) -> bool {
        matches!(self, KernelObject::ReadableEvent(_))
    }

    /// Evaluates if the object is in signaled state.
    pub fn is_signaled(&self) -> bool {
        match self {
            KernelObject::ReadableEvent(event) => event.is_signaled(),
            _ => false,
        }
    }

    /// Attempts to consume the signal for synchronization.
    pub fn try_consume_signal(&self) -> bool {
        match self {
            KernelObject::ReadableEvent(event) => event.try_consume_signal(),
            _ => false,
        }
    }
}
