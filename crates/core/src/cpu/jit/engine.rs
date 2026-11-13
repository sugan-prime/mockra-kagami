//! High-Performance JIT Execution Engine.
//!
//! Executes guest AArch64 code via compiled BasicBlocks with automatic
//! fallback to the pure interpreter for unsupported or dynamic opcodes.

use super::block::BlockOutcome;
use super::cache::BlockCache;
use crate::cpu::cpu::{Cpu, CpuError, StepResult};
use crate::memory::bus::{MemoryBus, MemoryError};
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum JitError {
    #[error("CPU interpreter error: {0}")]
    Cpu(#[from] CpuError),
    #[error("Memory error: {0}")]
    Memory(#[from] MemoryError),
    #[error("Compilation error: {0}")]
    Compile(String),
}

#[derive(Debug, Default)]
pub struct EngineMetrics {
    pub jit_instructions: u64,
    pub fallback_instructions: u64,
    pub blocks_executed: u64,
}

pub struct JitEngine {
    pub cache: BlockCache,
    pub metrics: EngineMetrics,
}

impl Default for JitEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl JitEngine {
    pub fn new() -> Self {
        Self {
            cache: BlockCache::new(),
            metrics: EngineMetrics::default(),
        }
    }

    /// Executes one basic block or a single interpreter instruction if fallback is required.
    pub fn step(&mut self, cpu: &mut Cpu, bus: &mut impl MemoryBus) -> Result<StepResult, JitError> {
        if cpu.is_halted {
            return Ok(StepResult::Halt);
        }

        let pc = cpu.regs.pc;

        // Fast path: get or compile BasicBlock for current PC
        let mut block = match self.cache.get_or_compile(pc, bus) {
            Ok(b) => b.clone(),
            Err(_) => {
                // If JIT compile fails, fall back to single interpreter instruction
                self.metrics.fallback_instructions += 1;
                return Ok(cpu.step(bus)?);
            }
        };

        // If block starts with a fallback instruction, step via interpreter
        if let super::block::BlockTerminal::Fallback { target_pc } = block.terminal {
            if target_pc == pc && block.micro_ops.is_empty() {
                self.metrics.fallback_instructions += 1;
                return Ok(cpu.step(bus)?);
            }
        }

        // Execute compiled micro-ops in the block
        let outcome = block.execute(&mut cpu.regs, bus)?;
        let count = block.instruction_count as u64;

        cpu.cycle_count = cpu.cycle_count.wrapping_add(count);
        self.metrics.jit_instructions += count;
        self.metrics.blocks_executed += 1;

        match outcome {
            BlockOutcome::Jump(next_pc) => {
                cpu.regs.pc = next_pc;
                Ok(StepResult::Continue)
            }
            BlockOutcome::Svc(imm, next_pc) => {
                cpu.regs.pc = next_pc;
                Ok(StepResult::Svc(imm))
            }
            BlockOutcome::Fallback(fallback_pc) => {
                cpu.regs.pc = fallback_pc;
                self.metrics.fallback_instructions += 1;
                Ok(cpu.step(bus)?)
            }
        }
    }

    /// Runs execution up to `max_cycles` or until CPU is halted.
    pub fn run(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut impl MemoryBus,
        max_cycles: u64,
    ) -> Result<StepResult, JitError> {
        let start_cycles = cpu.cycle_count;

        while cpu.cycle_count.wrapping_sub(start_cycles) < max_cycles {
            let res = self.step(cpu, bus)?;
            if res != StepResult::Continue {
                return Ok(res);
            }
        }

        Ok(StepResult::Continue)
    }
}
