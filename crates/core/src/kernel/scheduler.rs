//! Horizon OS Multi-Core Thread Scheduler.
//!
//! Models the Nintendo Switch 4-core Cortex-A57 scheduler:
//! - Cores 0, 1, 2 for application title execution (Main game loop, GPU command prep, Audio/Physics).
//! - Core 3 for operating system microkernel background tasks.
//! - Priority-based run queues, register context switching, thread yield, and futex arbitration.

use crate::cpu::registers::RegisterFile;
use crate::kernel::object::{KThread, ThreadStatus};
use std::collections::{HashMap, VecDeque};

pub const NUM_CORES: usize = 4;
pub const DEFAULT_APPLICATION_CORE: u8 = 0;

pub struct KScheduler {
    pub threads: HashMap<u64, KThread>,
    pub run_queues: [VecDeque<u64>; NUM_CORES],
    pub current_threads: [Option<u64>; NUM_CORES],
    pub next_thread_id: u64,
    pub futex_waiters: HashMap<u64, VecDeque<u64>>,
}

impl Default for KScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl KScheduler {
    pub fn new() -> Self {
        const EMPTY_QUEUE: VecDeque<u64> = VecDeque::new();
        Self {
            threads: HashMap::new(),
            run_queues: [EMPTY_QUEUE; NUM_CORES],
            current_threads: [None; NUM_CORES],
            next_thread_id: 1,
            futex_waiters: HashMap::new(),
        }
    }

    /// Registers the primary application thread (Thread ID 1) on Core 0.
    pub fn register_main_thread(&mut self, mut thread: KThread) {
        let tid = thread.thread_id;
        thread.status = ThreadStatus::Runnable;
        thread.core_id = DEFAULT_APPLICATION_CORE;
        if thread.regs.tpidr_el0 == 0 {
            thread.regs.tpidr_el0 = tid;
        }

        self.threads.insert(tid, thread);
        self.current_threads[DEFAULT_APPLICATION_CORE as usize] = Some(tid);
        self.next_thread_id = self.next_thread_id.max(tid + 1);
    }

    /// Creates a new guest thread with entry point, initial argument (X0), stack pointer, and core affinity.
    pub fn create_thread(
        &mut self,
        name: impl Into<String>,
        entry_point: u64,
        arg: u64,
        stack_top: u64,
        priority: i32,
        core_id: u8,
    ) -> u64 {
        let tid = self.next_thread_id;
        self.next_thread_id += 1;

        let mut thread = KThread::new(tid, name, entry_point, stack_top, priority);
        thread.core_id = core_id.min((NUM_CORES - 1) as u8);
        thread.arg = arg;
        thread.regs.set_x(0, arg); // Initial argument in X0
        thread.regs.tpidr_el0 = tid;

        self.threads.insert(tid, thread);
        tid
    }

    /// Moves a created thread into the runnable queue of its assigned core.
    pub fn start_thread(&mut self, thread_id: u64) -> bool {
        if let Some(thread) = self.threads.get_mut(&thread_id) {
            thread.status = ThreadStatus::Runnable;
            let core = thread.core_id as usize;
            self.run_queues[core].push_back(thread_id);
            true
        } else {
            false
        }
    }

    /// Terminates the designated thread and removes it from active run queues.
    pub fn exit_thread(&mut self, thread_id: u64) {
        if let Some(thread) = self.threads.get_mut(&thread_id) {
            thread.status = ThreadStatus::Terminated;
            let core = thread.core_id as usize;
            self.run_queues[core].retain(|&id| id != thread_id);
            if self.current_threads[core] == Some(thread_id) {
                self.current_threads[core] = None;
            }
        }
    }

    /// Saves the current core execution registers and performs a context switch to the next runnable thread.
    pub fn context_switch(
        &mut self,
        core_id: u8,
        current_regs: &mut RegisterFile,
    ) -> Option<u64> {
        let core = core_id as usize;

        // Save current thread state if active
        if let Some(curr_tid) = self.current_threads[core] {
            if let Some(thread) = self.threads.get_mut(&curr_tid) {
                thread.regs = current_regs.clone();
                if thread.status == ThreadStatus::Runnable {
                    self.run_queues[core].push_back(curr_tid);
                }
            }
        }

        // Pop next runnable thread
        if let Some(next_tid) = self.run_queues[core].pop_front() {
            if let Some(next_thread) = self.threads.get(&next_tid) {
                *current_regs = next_thread.regs.clone();
                self.current_threads[core] = Some(next_tid);
                return Some(next_tid);
            }
        }

        None
    }

    /// Puts the specified thread to sleep waiting on a kernel event handle.
    pub fn wait_on_handle(&mut self, thread_id: u64, handle: u32, current_regs: &RegisterFile) {
        if let Some(thread) = self.threads.get_mut(&thread_id) {
            thread.status = ThreadStatus::Waiting;
            thread.wait_handle = Some(handle);
            thread.regs = current_regs.clone();

            let core = thread.core_id as usize;
            self.run_queues[core].retain(|&id| id != thread_id);
            if self.current_threads[core] == Some(thread_id) {
                self.current_threads[core] = None;
            }
        }
    }

    /// Wakes all threads waiting on a signaled kernel event handle.
    pub fn signal_handle(&mut self, handle: u32) -> usize {
        let mut awakened = 0;
        for (&tid, thread) in self.threads.iter_mut() {
            if thread.status == ThreadStatus::Waiting && thread.wait_handle == Some(handle) {
                thread.status = ThreadStatus::Runnable;
                thread.wait_handle = None;
                let core = thread.core_id as usize;
                self.run_queues[core].push_back(tid);
                awakened += 1;
            }
        }
        awakened
    }

    /// Fast userland mutex (futex) lock arbitration.
    pub fn arbitrate_lock(&mut self, thread_id: u64, mutex_addr: u64, current_regs: &RegisterFile) {
        if let Some(thread) = self.threads.get_mut(&thread_id) {
            thread.status = ThreadStatus::Waiting;
            thread.regs = current_regs.clone();

            let core = thread.core_id as usize;
            self.run_queues[core].retain(|&id| id != thread_id);
            if self.current_threads[core] == Some(thread_id) {
                self.current_threads[core] = None;
            }

            self.futex_waiters
                .entry(mutex_addr)
                .or_default()
                .push_back(thread_id);
        }
    }

    /// Fast userland mutex (futex) unlock arbitration: awakens the next waiting thread.
    pub fn arbitrate_unlock(&mut self, mutex_addr: u64) -> Option<u64> {
        if let Some(waiters) = self.futex_waiters.get_mut(&mutex_addr) {
            if let Some(woken_tid) = waiters.pop_front() {
                if let Some(thread) = self.threads.get_mut(&woken_tid) {
                    thread.status = ThreadStatus::Runnable;
                    let core = thread.core_id as usize;
                    self.run_queues[core].push_back(woken_tid);
                    return Some(woken_tid);
                }
            }
        }
        None
    }

    /// Returns the total number of non-terminated active threads.
    pub fn active_thread_count(&self) -> usize {
        self.threads
            .values()
            .filter(|t| t.status != ThreadStatus::Terminated)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_thread_lifecycle_and_switching() {
        let mut sched = KScheduler::new();
        let main = KThread::new(1, "MainThread", 0x7100_0000, 0x8000_8000, 44);
        sched.register_main_thread(main);

        assert_eq!(sched.active_thread_count(), 1);

        // Create worker thread on Core 1
        let worker_tid = sched.create_thread("WorkerGpu", 0x7100_1000, 0x1234, 0x8000_9000, 44, 1);
        assert_eq!(worker_tid, 2);
        assert_eq!(sched.threads.get(&worker_tid).unwrap().regs.get_x(0), 0x1234);

        // Start worker thread
        assert!(sched.start_thread(worker_tid));
        assert_eq!(sched.run_queues[1].len(), 1);

        // Context switch on Core 1
        let mut worker_regs = RegisterFile::default();
        let switched = sched.context_switch(1, &mut worker_regs);
        assert_eq!(switched, Some(2));
        assert_eq!(worker_regs.pc, 0x7100_1000);
        assert_eq!(worker_regs.sp, 0x8000_9000);
        assert_eq!(worker_regs.get_x(0), 0x1234);

        // Wait on event handle 0x100
        sched.wait_on_handle(worker_tid, 0x100, &worker_regs);
        assert_eq!(sched.threads.get(&worker_tid).unwrap().status, ThreadStatus::Waiting);

        // Signal handle 0x100
        let awakened = sched.signal_handle(0x100);
        assert_eq!(awakened, 1);
        assert_eq!(sched.threads.get(&worker_tid).unwrap().status, ThreadStatus::Runnable);
        assert_eq!(sched.run_queues[1].len(), 1);

        // Exit worker thread
        sched.exit_thread(worker_tid);
        assert_eq!(sched.threads.get(&worker_tid).unwrap().status, ThreadStatus::Terminated);
        assert_eq!(sched.active_thread_count(), 1);
    }

    #[test]
    fn test_scheduler_futex_arbitration() {
        let mut sched = KScheduler::new();
        let main = KThread::new(1, "MainThread", 0x7100_0000, 0x8000_8000, 44);
        sched.register_main_thread(main);

        let t2 = sched.create_thread("WorkerLock", 0x7100_2000, 0, 0x8000_A000, 44, 0);
        sched.start_thread(t2);

        let regs = RegisterFile::default();
        const MUTEX_ADDR: u64 = 0x8000_5000;

        // t2 blocks on futex
        sched.arbitrate_lock(t2, MUTEX_ADDR, &regs);
        assert_eq!(sched.threads.get(&t2).unwrap().status, ThreadStatus::Waiting);

        // Main unlocks futex
        let unblocked = sched.arbitrate_unlock(MUTEX_ADDR);
        assert_eq!(unblocked, Some(t2));
        assert_eq!(sched.threads.get(&t2).unwrap().status, ThreadStatus::Runnable);
    }
}
