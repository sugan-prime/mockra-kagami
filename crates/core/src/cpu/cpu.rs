//! AArch64 CPU Execution Loop, Memory Access, and Opcode Dispatch.

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
        let result = self.execute_instruction(instr, bus, &mut branched)?;

        // 3. Advance PC if no branch occurred
        if !branched {
            self.regs.pc = self.regs.pc.wrapping_add(4);
        }

        self.cycle_count = self.cycle_count.wrapping_add(1);

        Ok(result)
    }

    fn execute_instruction(
        &mut self,
        instr: Instruction,
        bus: &mut impl MemoryBus,
        branched: &mut bool,
    ) -> Result<StepResult, CpuError> {
        // Top-level decode based on AArch64 architecture manual bits [28:25]
        let op0 = instr.bits(25, 28);

        match op0 {
            // Loads and Stores: x1x0 -> 0100, 0110, 1100, 1110
            0b0100 | 0b0110 | 0b1100 | 0b1110 => self.execute_load_store(instr, bus),

            // Branches, Exception Generating and System Instructions: 101x
            0b1010 | 0b1011 => self.execute_branch_or_system(instr, branched),

            // Data Processing -- Immediate: 100x
            0b1000 | 0b1001 => self.execute_data_processing_imm(instr),

            // Data Processing -- Register: 0101, 1101
            0b0101 | 0b1101 => self.execute_data_processing_reg(instr),

            _ => Err(CpuError::IllegalInstruction(instr.0, self.regs.pc)),
        }
    }

    // =========================================================================
    // Control Flow: Branches, System, SVC
    // =========================================================================

    /// Evaluates condition codes (bits [3:0]) against current PSTATE.NZCV flags.
    pub fn evaluate_condition(&self, cond: u32) -> bool {
        let pstate = self.regs.pstate;
        match cond {
            0b0000 => pstate.z,                                 // EQ: Equal
            0b0001 => !pstate.z,                                // NE: Not Equal
            0b0010 => pstate.c,                                 // CS / HS: Carry Set / Unsigned higher or same
            0b0011 => !pstate.c,                                // CC / LO: Carry Clear / Unsigned lower
            0b0100 => pstate.n,                                 // MI: Minus / Negative
            0b0101 => !pstate.n,                                // PL: Plus / Positive or zero
            0b0110 => pstate.v,                                 // VS: Overflow
            0b0111 => !pstate.v,                                // VC: No Overflow
            0b1000 => pstate.c && !pstate.z,                    // HI: Unsigned higher
            0b1001 => !(pstate.c && !pstate.z),                 // LS: Unsigned lower or same
            0b1010 => pstate.n == pstate.v,                     // GE: Signed greater than or equal
            0b1011 => pstate.n != pstate.v,                     // LT: Signed less than
            0b1100 => !pstate.z && (pstate.n == pstate.v),         // GT: Signed greater than
            0b1101 => !(!pstate.z && (pstate.n == pstate.v)), // LE: Signed less than or equal
            0b1110 | 0b1111 => true,                            // AL / NV: Always
            _ => unreachable!(),
        }
    }

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

        // Conditional branch: B.cond: bits [31:24] == 01010100 and bit [4] == 0
        if instr.bits(24, 31) == 0b01010100 && !instr.bit(4) {
            let cond = instr.cond();
            if self.evaluate_condition(cond) {
                let imm19 = instr.bits(5, 23) as u64;
                let offset = sign_extend_64(imm19, 19) * 4;
                self.regs.pc = (self.regs.pc as i64).wrapping_add(offset) as u64;
                *branched = true;
            }
            return Ok(StepResult::Continue);
        }

        // Compare and branch: CBZ (bit 24 == 0) and CBNZ (bit 24 == 1): bits [30:25] == 011010
        if instr.bits(25, 30) == 0b011010 {
            let is_64 = instr.is_64bit();
            let is_cbnz = instr.bit(24);
            let rt = instr.rd();
            let val = if is_64 {
                self.regs.get_x(rt)
            } else {
                self.regs.get_w(rt) as u64
            };

            let take_branch = if is_cbnz { val != 0 } else { val == 0 };
            if take_branch {
                let imm19 = instr.bits(5, 23) as u64;
                let offset = sign_extend_64(imm19, 19) * 4;
                self.regs.pc = (self.regs.pc as i64).wrapping_add(offset) as u64;
                *branched = true;
            }
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

    // =========================================================================
    // Memory: Loads and Stores (LDR, STR, LDP, STP)
    // =========================================================================

    fn execute_load_store(&mut self, instr: Instruction, bus: &mut impl MemoryBus) -> Result<StepResult, CpuError> {
        // 1. Load/Store Register Pair (LDP / STP): bits [27:29] == 101 and V == 0 (bit 26 == 0)
        if instr.bits(27, 29) == 0b101 && !instr.bit(26) {
            return self.execute_ldp_stp(instr, bus);
        }

        // 2. Load/Store Single Register (Unsigned Immediate Offset): bits [24:29] == 111001 and V == 0
        if instr.bits(24, 29) == 0b111001 && !instr.bit(26) {
            return self.execute_ldr_str_unsigned_imm(instr, bus);
        }

        // 3. Load/Store Single Register (Unscaled / Pre-Index / Post-Index): bits [24:29] == 111000 and V == 0
        if instr.bits(24, 29) == 0b111000 && !instr.bit(26) {
            return self.execute_ldr_str_indexed(instr, bus);
        }

        Err(CpuError::IllegalInstruction(instr.0, self.regs.pc))
    }

    fn execute_ldp_stp(&mut self, instr: Instruction, bus: &mut impl MemoryBus) -> Result<StepResult, CpuError> {
        let is_64 = instr.bits(30, 31) == 0b10;
        let is_load = instr.bit(22);
        let mode = instr.bits(23, 25);
        let imm7 = instr.bits(15, 21) as u64;
        let rn = instr.rn();
        let rt1 = instr.rd();
        let rt2 = instr.bits(10, 14) as usize;

        let scale = if is_64 { 8 } else { 4 };
        let offset = sign_extend_64(imm7, 7) * scale;
        let base = self.regs.get_reg_or_sp(rn);

        let target_addr = match mode {
            // Post-index: bits [25:23] == 001
            0b001 => {
                let new_base = (base as i64).wrapping_add(offset) as u64;
                self.regs.set_reg_or_sp(rn, new_base);
                base
            }
            // Pre-index: bits [25:23] == 011
            0b011 => {
                let new_base = (base as i64).wrapping_add(offset) as u64;
                self.regs.set_reg_or_sp(rn, new_base);
                new_base
            }
            // Signed offset: bits [25:23] == 010
            0b010 => (base as i64).wrapping_add(offset) as u64,
            _ => return Err(CpuError::IllegalInstruction(instr.0, self.regs.pc)),
        };

        if is_load {
            if is_64 {
                let val1 = bus.read_u64(target_addr)?;
                let val2 = bus.read_u64(target_addr.wrapping_add(8))?;
                self.regs.set_x(rt1, val1);
                self.regs.set_x(rt2, val2);
            } else {
                let val1 = bus.read_u32(target_addr)?;
                let val2 = bus.read_u32(target_addr.wrapping_add(4))?;
                self.regs.set_w(rt1, val1);
                self.regs.set_w(rt2, val2);
            }
        } else {
            // Store Pair
            if is_64 {
                bus.write_u64(target_addr, self.regs.get_x(rt1))?;
                bus.write_u64(target_addr.wrapping_add(8), self.regs.get_x(rt2))?;
            } else {
                bus.write_u32(target_addr, self.regs.get_w(rt1))?;
                bus.write_u32(target_addr.wrapping_add(4), self.regs.get_w(rt2))?;
            }
        }

        Ok(StepResult::Continue)
    }

    fn execute_ldr_str_unsigned_imm(&mut self, instr: Instruction, bus: &mut impl MemoryBus) -> Result<StepResult, CpuError> {
        let size = instr.bits(30, 31);
        let opc = instr.bits(22, 23);
        let imm12 = instr.bits(10, 21) as u64;
        let rn = instr.rn();
        let rt = instr.rd();

        let offset = imm12 << size;
        let addr = self.regs.get_reg_or_sp(rn).wrapping_add(offset);

        self.perform_single_load_store(size, opc, rt, addr, bus)
    }

    fn execute_ldr_str_indexed(&mut self, instr: Instruction, bus: &mut impl MemoryBus) -> Result<StepResult, CpuError> {
        let size = instr.bits(30, 31);
        let opc = instr.bits(22, 23);
        let imm9 = instr.bits(12, 20) as u64;
        let mode = instr.bits(10, 11);
        let rn = instr.rn();
        let rt = instr.rd();

        let offset = sign_extend_64(imm9, 9);
        let base = self.regs.get_reg_or_sp(rn);

        let target_addr = match mode {
            // Unscaled immediate (LDUR / STUR)
            0b00 => (base as i64).wrapping_add(offset) as u64,
            // Post-indexed: Rn := Rn + offset after memory access
            0b01 => {
                let new_base = (base as i64).wrapping_add(offset) as u64;
                self.regs.set_reg_or_sp(rn, new_base);
                base
            }
            // Pre-indexed: Rn := Rn + offset before memory access
            0b11 => {
                let new_base = (base as i64).wrapping_add(offset) as u64;
                self.regs.set_reg_or_sp(rn, new_base);
                new_base
            }
            _ => return Err(CpuError::IllegalInstruction(instr.0, self.regs.pc)),
        };

        self.perform_single_load_store(size, opc, rt, target_addr, bus)
    }

    fn perform_single_load_store(
        &mut self,
        size: u32,
        opc: u32,
        rt: usize,
        addr: u64,
        bus: &mut impl MemoryBus,
    ) -> Result<StepResult, CpuError> {
        match (size, opc) {
            // 64-bit X
            (0b11, 0b00) => bus.write_u64(addr, self.regs.get_x(rt))?,
            (0b11, 0b01) => self.regs.set_x(rt, bus.read_u64(addr)?),

            // 32-bit W
            (0b10, 0b00) => bus.write_u32(addr, self.regs.get_w(rt))?,
            (0b10, 0b01) => self.regs.set_w(rt, bus.read_u32(addr)?),
            (0b10, 0b10) => self.regs.set_x(rt, bus.read_u32(addr)? as i32 as i64 as u64), // LDRSW

            // 16-bit Halfword
            (0b01, 0b00) => bus.write_u16(addr, self.regs.get_w(rt) as u16)?,
            (0b01, 0b01) => self.regs.set_w(rt, bus.read_u16(addr)? as u32),
            (0b01, 0b10) => self.regs.set_x(rt, bus.read_u16(addr)? as i16 as i64 as u64), // LDRSH (64)
            (0b01, 0b11) => self.regs.set_w(rt, bus.read_u16(addr)? as i16 as i32 as u32), // LDRSH (32)

            // 8-bit Byte
            (0b00, 0b00) => bus.write_u8(addr, self.regs.get_w(rt) as u8)?,
            (0b00, 0b01) => self.regs.set_w(rt, bus.read_u8(addr)? as u32),
            (0b00, 0b10) => self.regs.set_x(rt, bus.read_u8(addr)? as i8 as i64 as u64), // LDRSB (64)
            (0b00, 0b11) => self.regs.set_w(rt, bus.read_u8(addr)? as i8 as i32 as u32), // LDRSB (32)

            _ => return Err(CpuError::IllegalInstruction(0, self.regs.pc)),
        }

        Ok(StepResult::Continue)
    }

    // =========================================================================
    // ALU: Data Processing Immediate & Register
    // =========================================================================

    fn execute_data_processing_imm(&mut self, instr: Instruction) -> Result<StepResult, CpuError> {
        let is_64 = instr.is_64bit();
        let opc = instr.bits(23, 25);

        match opc {
            // Add/Subtract (immediate): bits [25:23] == 010
            0b010 => {
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
                    // For flag-setting operations (CMP, ADDS, SUBS), register 31 means XZR/WZR
                    if is_64 {
                        self.regs.set_x(instr.rd(), result);
                    } else {
                        self.regs.set_w(instr.rd(), result as u32);
                    }
                } else {
                    // For non-flag setting operations, register 31 represents SP
                    self.regs.set_reg_or_sp(instr.rd(), result);
                }

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

    fn execute_data_processing_reg(&mut self, instr: Instruction) -> Result<StepResult, CpuError> {
        let is_64 = instr.is_64bit();

        // 1. Add/Subtract (shifted register): bits [28:24] == 01011
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

        // 2. Logical (shifted register): AND, ORR, EOR, ANDS: bits [28:24] == 01010
        if instr.bits(24, 28) == 0b01010 {
            let opc = instr.bits(29, 30);
            let invert = instr.bit(21);
            let rm = instr.rm();
            let rn = instr.rn();
            let rd = instr.rd();

            let mut rm_val = if is_64 { self.regs.get_x(rm) } else { self.regs.get_w(rm) as u64 };
            let rn_val = if is_64 { self.regs.get_x(rn) } else { self.regs.get_w(rn) as u64 };

            if invert {
                rm_val = !rm_val;
            }

            let (result, set_flags) = match opc {
                0b00 => (rn_val & rm_val, false), // AND / BIC
                0b01 => (rn_val | rm_val, false), // ORR / ORN (e.g. MOV Rd, Rm)
                0b10 => (rn_val ^ rm_val, false), // EOR / EON
                0b11 => (rn_val & rm_val, true),  // ANDS / BICS (e.g. TST)
                _ => unreachable!(),
            };

            if set_flags {
                if is_64 {
                    self.regs.pstate.n = (result as i64) < 0;
                    self.regs.pstate.z = result == 0;
                } else {
                    self.regs.pstate.n = (result as i32) < 0;
                    self.regs.pstate.z = (result as u32) == 0;
                }
                self.regs.pstate.c = false;
                self.regs.pstate.v = false;
            }

            if is_64 {
                self.regs.set_x(rd, result);
            } else {
                self.regs.set_w(rd, result as u32);
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
                self.regs.pstate.v = (!(op1 ^ op2) & (op1 ^ res) & 0x8000_0000_0000_0000) != 0;
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
                self.regs.pstate.v = (!(o1_32 ^ o2_32) & (o1_32 ^ r32) & 0x8000_0000) != 0;
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

    #[test]
    fn test_memory_load_and_store() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x2000);
        let mut cpu = Cpu::new();
        cpu.reset(0x8000_0000);

        // 1. MOVZ X0, #0x1234
        // 2. MOVZ X1, #0x8000, LSL #16 (X1 = 0x8000_1000 base)
        //    MOVK X1, #0x1000
        // 3. STR X0, [X1, #0]
        // 4. LDR X2, [X1, #0]
        // 5. SVC #1
        let code: [u32; 6] = [
            0xD2824680, // movz x0, #0x1234
            0xD2B00001, // movz x1, #0x8000, lsl #16 (x1 = 0x8000_0000)
            0xF2820001, // movk x1, #0x1000          (x1 = 0x8000_1000)
            0xF9000020, // str x0, [x1, #0]
            0xF9400022, // ldr x2, [x1, #0]
            0xD4000021, // svc #1
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        loop {
            match cpu.step(&mut mem).unwrap() {
                StepResult::Continue => continue,
                StepResult::Svc(1) => break,
                other => panic!("Unexpected result: {:?}", other),
            }
        }

        assert_eq!(cpu.regs.get_x(0), 0x1234);
        assert_eq!(cpu.regs.get_x(1), 0x8000_1000);
        assert_eq!(cpu.regs.get_x(2), 0x1234);
        assert_eq!(mem.read_u64(0x8000_1000).unwrap(), 0x1234);
    }

    #[test]
    fn test_loop_with_conditional_branch() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x1000);
        let mut cpu = Cpu::new();
        cpu.reset(0x8000_0000);

        // Loop countdown test:
        //   MOVZ X0, #5          ; X0 = 5 (loop counter)
        //   MOVZ X1, #0          ; X1 = 0 (accumulator)
        // loop:
        //   ADD X1, X1, #10      ; X1 += 10
        //   SUBS X0, X0, #1      ; X0 -= 1, update flags
        //   B.NE loop            ; if X0 != 0 (Z == 0), branch back
        //   SVC #99              ; Done
        let code: [u32; 6] = [
            0xD28000A0, // movz x0, #5
            0xD2800001, // movz x1, #0
            0x91002821, // add x1, x1, #10
            0xF1000400, // subs x0, x0, #1
            0x54FFFFC1, // b.ne -2 (back to add: offset = -2 instrs = -8 bytes)
            0xD4000C61, // svc #99
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        loop {
            match cpu.step(&mut mem).unwrap() {
                StepResult::Continue => continue,
                StepResult::Svc(99) => break,
                other => panic!("Unexpected result: {:?}", other),
            }
        }

        assert_eq!(cpu.regs.get_x(0), 0);
        assert_eq!(cpu.regs.get_x(1), 50); // 5 iterations * 10 = 50!
        assert!(cpu.regs.pstate.z); // Z flag should be set
    }

    #[test]
    fn test_cbz_and_cbnz() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x1000);
        let mut cpu = Cpu::new();
        cpu.reset(0x8000_0000);

        // 1. MOVZ X0, #0
        // 2. CBZ X0, +2 (skip next instruction)
        // 3. MOVZ X1, #0xDEAD (should be skipped!)
        // 4. MOVZ X1, #0xBEEF
        // 5. SVC #7
        let code: [u32; 5] = [
            0xD2800000, // movz x0, #0
            0xB4000040, // cbz x0, +2 (offset = +8 bytes)
            0xD29BD5A1, // movz x1, #0xDEAD
            0xD297DDE1, // movz x1, #0xBEEF
            0xD40000E1, // svc #7
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        loop {
            match cpu.step(&mut mem).unwrap() {
                StepResult::Continue => continue,
                StepResult::Svc(7) => break,
                other => panic!("Unexpected result: {:?}", other),
            }
        }

        assert_eq!(cpu.regs.get_x(1), 0xBEEF);
    }

    #[test]
    fn test_ldp_and_stp_stack_frames() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x10000);
        let mut cpu = Cpu::new();
        cpu.reset(0x8000_0000);
        // Set SP to 0x8000_8000
        cpu.regs.sp = 0x8000_8000;

        // Sequence:
        // 1. MOVZ X0, #0x1111
        // 2. MOVZ X1, #0x2222
        // 3. STP X0, X1, [SP, #-16]!  ; Pre-index: SP := SP - 16, store [SP]=X0, [SP+8]=X1
        // 4. LDP X2, X3, [SP], #16    ; Post-index: load X2=[SP], X3=[SP+8], SP := SP + 16
        // 5. SVC #5
        let code: [u32; 5] = [
            0xD2822220, // movz x0, #0x1111
            0xD2844441, // movz x1, #0x2222
            0xA9BF07E0, // stp x0, x1, [sp, #-16]!
            0xA8C10FE2, // ldp x2, x3, [sp], #16
            0xD40000A1, // svc #5
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        loop {
            match cpu.step(&mut mem).unwrap() {
                StepResult::Continue => continue,
                StepResult::Svc(5) => break,
                other => panic!("Unexpected result: {:?}", other),
            }
        }

        assert_eq!(cpu.regs.get_x(2), 0x1111);
        assert_eq!(cpu.regs.get_x(3), 0x2222);
        assert_eq!(cpu.regs.sp, 0x8000_8000); // SP successfully restored
    }

    #[test]
    fn test_cmp_immediate_and_b_eq() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x1000);
        let mut cpu = Cpu::new();
        cpu.reset(0x8000_0000);

        // Sequence:
        // 1. MOVZ X0, #42
        // 2. CMP X0, #42        ; subs xzr, x0, #42 (flags set: Z=1)
        // 3. B.EQ match         ; should branch to match
        // 4. MOVZ X1, #0xFAIL   ; skipped
        // match:
        // 5. MOVZ X1, #0xPASS
        // 6. SVC #10
        let code: [u32; 6] = [
            0xD2800540, // movz x0, #42
            0xF100A81F, // cmp x0, #42 (subs xzr, x0, #42)
            0x54000040, // b.eq +2 (offset = +8 bytes)
            0xD2800021, // movz x1, #1 (FAIL)
            0xD2800041, // movz x1, #2 (PASS)
            0xD4000141, // svc #10
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        loop {
            match cpu.step(&mut mem).unwrap() {
                StepResult::Continue => continue,
                StepResult::Svc(10) => break,
                other => panic!("Unexpected result: {:?}", other),
            }
        }

        assert_eq!(cpu.regs.get_x(1), 2); // Branch was taken, PASS recorded!
        assert!(cpu.regs.pstate.z);
    }
}
