//! Horizon OS Process Handle Table.
//!
//! Tracks active handles (events, sessions, threads, processes) and routes
//! system calls to the underlying KernelObject instances.

use crate::kernel::ipc::ServiceHandler;
use crate::kernel::object::{KEvent, KSession, KernelObject, CURRENT_PROCESS_PSEUDO_HANDLE, CURRENT_THREAD_PSEUDO_HANDLE};
use std::collections::HashMap;
use std::sync::Arc;

pub struct HandleTable {
    next_handle: u32,
    entries: HashMap<u32, KernelObject>,
    pub current_process: Option<KernelObject>,
    pub current_thread: Option<KernelObject>,
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            next_handle: 0x01,
            entries: HashMap::new(),
            current_process: None,
            current_thread: None,
        }
    }

    /// Allocates an entry for a kernel object and returns a new 32-bit handle.
    pub fn insert(&mut self, object: KernelObject) -> u32 {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1);
        if self.next_handle == 0 || self.next_handle >= 0xFFFF_0000 {
            self.next_handle = 1;
        }
        self.entries.insert(handle, object);
        handle
    }

    /// Looks up a kernel object by its handle, with pseudo-handle resolution.
    pub fn get(&self, handle: u32) -> Option<&KernelObject> {
        if handle == CURRENT_PROCESS_PSEUDO_HANDLE {
            return self.current_process.as_ref();
        }
        if handle == CURRENT_THREAD_PSEUDO_HANDLE {
            return self.current_thread.as_ref();
        }
        self.entries.get(&handle)
    }

    /// Closes and drops the kernel object associated with `handle`.
    pub fn close(&mut self, handle: u32) -> bool {
        self.entries.remove(&handle).is_some()
    }

    /// Creates a connected pair of readable and writable event handles.
    pub fn create_event_pair(&mut self, auto_reset: bool) -> (u32, u32) {
        let event = KEvent::new(auto_reset);
        let readable = self.insert(KernelObject::ReadableEvent(event.clone()));
        let writable = self.insert(KernelObject::WritableEvent(event));
        (readable, writable)
    }

    /// Creates an IPC session handle connected to an HLE service handler.
    pub fn create_session(&mut self, service_name: impl Into<String>, handler: Arc<dyn ServiceHandler>) -> u32 {
        let session = KSession {
            service_name: service_name.into(),
            handler,
        };
        self.insert(KernelObject::Session(session))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_table_event_pair() {
        let mut table = HandleTable::new();
        let (readable, writable) = table.create_event_pair(true);

        assert_ne!(readable, writable);

        // Readable is not signaled initially
        let read_obj = table.get(readable).unwrap();
        assert!(!read_obj.is_signaled());

        // Signal via writable end
        if let Some(KernelObject::WritableEvent(w)) = table.get(writable) {
            w.signal();
        } else {
            panic!("Expected WritableEvent");
        }

        // Readable should now be signaled
        let read_obj = table.get(readable).unwrap();
        assert!(read_obj.is_signaled());

        // Consuming signal (auto-reset) resets it to unsignaled
        assert!(read_obj.try_consume_signal());
        assert!(!read_obj.is_signaled());
    }
}
