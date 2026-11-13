//! AArch64 CPU Execution Loop and Opcode Dispatch.

use crate::cpu::instruction::Instruction;
use crate::cpu::registers::RegisterFile;
use crate::memory::bus::{MemoryBus, MemoryError};
use common::sign_extend_64;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum CpuError {
    #[error("Memory fault: {0}")]
    MemoryFault(#[from] MemoryError),
    #[error("Illegal instruction: 0x{0:08X} at PC: 0x{1:016X}")]
    IllegalInstruction(u32, u64),
}

#[derive(Debug, PartialEq, Eq)]
pub enum StepResult {
    /// Normal execution, PC advanced.
    Continue,
    /// Halted by WFI or explicit halt instruction.
    Halt,
    /// Breakpoint hit.
    Breakpoint,
    /// Supervisor Call (SVC) invoked with the given 16-bit immediate.
    Svc(u16),
}

pub struct Cpu {
    pub regs: RegisterFile,
    pub cycle_count: u64,
    pub is_halted: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            regs: RegisterFile::default(),
            cycle_count: 0,
            is_halted: false,
        }
    }

    /// Resets CPU state and sets the initial Program Counter.
    pub fn reset(&mut self, entry_point: u64) {
        self.regs = RegisterFile::default();
        self.regs.pc = entry_point;
        self.cycle_count = 0;
        self.is_halted = false;
    }

    /// Executes a single instruction cycle: Fetch -> Decode -> Execute.
    pub fn step(&mut self, bus: &mut impl MemoryBus) -> Result<StepResult, CpuError> {
        if self.is_halted {
            return Ok(StepResult::Halt);
        }

        // 1. Fetch
        let pc = self.regs.pc;
        let raw_instr = bus.read_u32(pc)?;
        let instr = Instruction(raw_instr);

        // Track whether the instruction explicitly branched/modified PC
        let mut branched = false;

        // 2. Decode & Execute
        let result = self.execute_instruction(instr, &mut branched)?;

        // 3. Advance PC if no branch occurred
        if !branched {
            self.regs.pc = self.regs.pc.wrapping_add(4);
        }

        self.cycle_count = self.cycle_count.wrapping_add(1);

        Ok(result)
    }

    fn execute_instruction(&mut self, instr: Instruction, branched: &mut bool) -> Result<StepResult, CpuError> {
        // Top-level decode based on AArch64 architecture manual bits [28:25]
        let op0 = instr.bits(25, 28);

        match op0 {
            // Branches, Exception Generating and System Instructions: 101x
            0b1010 | 0b1011 => self.execute_branch_or_system(instr, branched),

            // Data Processing -- Immediate: 100x
            0b1000 | 0b1001 => self.execute_data_processing_imm(instr),

            // Data Processing -- Register: 0101, 1101
            0b0101 | 0b1101 => self.execute_data_processing_reg(instr),

            _ => Err(CpuError::IllegalInstruction(instr.0, self.regs.pc)),
        }
    }

    /// Handles Unconditional Branches, RET, SVC, etc.
    fn execute_branch_or_system(&mut self, instr: Instruction, branched: &mut bool) -> Result<StepResult, CpuError> {
        // Unconditional branch (immediate): B: bits [31:26] == 000101
        if instr.bits(26, 31) == 0b000101 {
            let imm26 = instr.bits(0, 25) as u64;
            let offset = sign_extend_64(imm26, 26) * 4;
            self.regs.pc = (self.regs.pc as i64).wrapping_add(offset) as u64;
            *branched = true;
            return Ok(StepResult::Continue);
        }

        // Unconditional branch with link: BL: bits [31:26] == 100101
        if instr.bits(26, 31) == 0b100101 {
            let imm26 = instr.bits(0, 25) as u64;
            let offset = sign_extend_64(imm26, 26) * 4;
            self.regs.set_x(30, self.regs.pc.wrapping_add(4)); // Store return address in LR (X30)
            self.regs.pc = (self.regs.pc as i64).wrapping_add(offset) as u64;
            *branched = true;
            return Ok(StepResult::Continue);
        }

        // RET: bits [31:10] == 1101011001011111000000
        if instr.bits(10, 31) == 0b1101011001011111000000 {
            let rn = instr.rn();
            let target = self.regs.get_x(rn);
            self.regs.pc = target;
            *branched = true;
            return Ok(StepResult::Continue);
        }

        // SVC (Supervisor Call): bits [31:21] == 11010100000 and bits [4:0] == 00001
        if instr.bits(21, 31) == 0b11010100000 && instr.bits(0, 4) == 0b00001 {
            let imm16 = instr.bits(5, 20) as u16;
            return Ok(StepResult::Svc(imm16));
        }

        Err(CpuError::IllegalInstruction(instr.0, self.regs.pc))
    }

    /// Handles immediate data processing: ADD, SUB, MOVZ, MOVN, MOVK.
    fn execute_data_processing_imm(&mut self, instr: Instruction) -> Result<StepResult, CpuError> {
        let is_64 = instr.is_64bit();
        let opc = instr.bits(23, 25);

        match opc {
            // Add/Subtract (immediate): bits [25:24] == 00
            0b000 | 0b001 => {
                let is_sub = instr.bit(30);
                let set_flags = instr.bit(29);
                let shift = instr.bits(22, 23);
                let imm12 = instr.bits(10, 21) as u64;
                let val = match shift {
                    0b00 => imm12,
                    0b01 => imm12 << 12,
                    _ => return Err(CpuError::IllegalInstruction(instr.0, self.regs.pc)),
                };

                let rn_val = self.regs.get_reg_or_sp(instr.rn());
                let result = if is_sub {
                    if is_64 {
                        rn_val.wrapping_sub(val)
                    } else {
                        (rn_val as u32).wrapping_sub(val as u32) as u64
                    }
                } else {
                    if is_64 {
                        rn_val.wrapping_add(val)
                    } else {
                        (rn_val as u32).wrapping_add(val as u32) as u64
                    }
                };

                if set_flags {
                    self.update_add_sub_flags(rn_val, val, result, is_sub, is_64);
                }

                // If destination is SP vs XZR:
                self.regs.set_reg_or_sp(instr.rd(), result);
                Ok(StepResult::Continue)
            }

            // Move Wide (immediate): MOVZ, MOVN, MOVK: bits [25:23] == 101
            0b101 => {
                let opc_wide = instr.bits(29, 30);
                let hw = instr.bits(21, 22);
                let imm16 = instr.bits(5, 20) as u64;
                let shift = hw * 16;
                let shifted_imm = imm16 << shift;

                match opc_wide {
                    // MOVZ (Move with zero): opc == 10
                    0b10 => {
                        if is_64 {
                            self.regs.set_x(instr.rd(), shifted_imm);
                        } else {
                            self.regs.set_w(instr.rd(), shifted_imm as u32);
                        }
                        Ok(StepResult::Continue)
                    }
                    // MOVN (Move with NOT): opc == 00
                    0b00 => {
                        let val = if is_64 { !shifted_imm } else { !(shifted_imm as u32) as u64 };
                        if is_64 {
                            self.regs.set_x(instr.rd(), val);
                        } else {
                            self.regs.set_w(instr.rd(), val as u32);
                        }
                        Ok(StepResult::Continue)
                    }
                    // MOVK (Move with keep): opc == 11
                    0b11 => {
                        let mask = !(0xFFFFu64 << shift);
                        let old_val = if is_64 { self.regs.get_x(instr.rd()) } else { self.regs.get_w(instr.rd()) as u64 };
                        let val = (old_val & mask) | shifted_imm;
                        if is_64 {
                            self.regs.set_x(instr.rd(), val);
                        } else {
                            self.regs.set_w(instr.rd(), val as u32);
                        }
                        Ok(StepResult::Continue)
                    }
                    _ => Err(CpuError::IllegalInstruction(instr.0, self.regs.pc)),
                }
            }

            _ => Err(CpuError::IllegalInstruction(instr.0, self.regs.pc)),
        }
    }

    /// Handles register-shifted data processing (e.g. ADD/SUB reg).
    fn execute_data_processing_reg(&mut self, instr: Instruction) -> Result<StepResult, CpuError> {
        let is_64 = instr.is_64bit();
        // Add/sub (shifted register): bits [28:24] == 01011
        if instr.bits(24, 28) == 0b01011 {
            let is_sub = instr.bit(30);
            let set_flags = instr.bit(29);
            let rm_val = self.regs.get_x(instr.rm());
            let rn_val = self.regs.get_x(instr.rn());

            let result = if is_sub {
                if is_64 {
                    rn_val.wrapping_sub(rm_val)
                } else {
                    (rn_val as u32).wrapping_sub(rm_val as u32) as u64
                }
            } else {
                if is_64 {
                    rn_val.wrapping_add(rm_val)
                } else {
                    (rn_val as u32).wrapping_add(rm_val as u32) as u64
                }
            };

            if set_flags {
                self.update_add_sub_flags(rn_val, rm_val, result, is_sub, is_64);
            }

            if is_64 {
                self.regs.set_x(instr.rd(), result);
            } else {
                self.regs.set_w(instr.rd(), result as u32);
            }
            return Ok(StepResult::Continue);
        }

        Err(CpuError::IllegalInstruction(instr.0, self.regs.pc))
    }

    fn update_add_sub_flags(&mut self, op1: u64, op2: u64, res: u64, is_sub: bool, is_64: bool) {
        if is_64 {
            self.regs.pstate.n = (res as i64) < 0;
            self.regs.pstate.z = res == 0;
            if is_sub {
                self.regs.pstate.c = op1 >= op2;
                self.regs.pstate.v = ((op1 ^ op2) & (op1 ^ res) & 0x8000_0000_0000_0000) != 0;
            } else {
                self.regs.pstate.c = res < op1;
                self.regs.pstate.v = (! (op1 ^ op2) & (op1 ^ res) & 0x8000_0000_0000_0000) != 0;
            }
        } else {
            let r32 = res as u32;
            let o1_32 = op1 as u32;
            let o2_32 = op2 as u32;
            self.regs.pstate.n = (r32 as i32) < 0;
            self.regs.pstate.z = r32 == 0;
            if is_sub {
                self.regs.pstate.c = o1_32 >= o2_32;
                self.regs.pstate.v = ((o1_32 ^ o2_32) & (o1_32 ^ r32) & 0x8000_0000) != 0;
            } else {
                self.regs.pstate.c = r32 < o1_32;
                self.regs.pstate.v = (! (o1_32 ^ o2_32) & (o1_32 ^ r32) & 0x8000_0000) != 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_movz_and_add_sequence() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x1000);
        let mut cpu = Cpu::new();
        cpu.reset(0x8000_0000);

        // Sequence:
        // 1. MOVZ X0, #42 (0xD2800540)
        // 2. MOVZ X1, #58 (0xD2800741)
        // 3. ADD X2, X0, X1 (0x8B010002)
        // 4. SVC #0 (0xD4000001)
        let code: [u32; 4] = [
            0xD2800540, // movz x0, #42
            0xD2800741, // movz x1, #58
            0x8B010002, // add x2, x0, x1
            0xD4000001, // svc #0
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        // Step 1: MOVZ X0, #42
        assert_eq!(cpu.step(&mut mem).unwrap(), StepResult::Continue);
        assert_eq!(cpu.regs.get_x(0), 42);
        assert_eq!(cpu.regs.pc, 0x8000_0004);

        // Step 2: MOVZ X1, #58
        assert_eq!(cpu.step(&mut mem).unwrap(), StepResult::Continue);
        assert_eq!(cpu.regs.get_x(1), 58);
        assert_eq!(cpu.regs.pc, 0x8000_0008);

        // Step 3: ADD X2, X0, X1 -> X2 = 100
        assert_eq!(cpu.step(&mut mem).unwrap(), StepResult::Continue);
        assert_eq!(cpu.regs.get_x(2), 100);
        assert_eq!(cpu.regs.pc, 0x8000_000C);

        // Step 4: SVC #0
        assert_eq!(cpu.step(&mut mem).unwrap(), StepResult::Svc(0));
    }
}
