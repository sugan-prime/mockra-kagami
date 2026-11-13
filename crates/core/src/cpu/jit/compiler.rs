//! AArch64 Basic Block JIT Compiler.
//!
//! Translates guest ARM64 machine words from memory into compiled BasicBlock IR.

use super::block::{BasicBlock, BlockTerminal, MicroOp};
use crate::cpu::instruction::Instruction;
use crate::memory::bus::{MemoryBus, MemoryError};
use common::sign_extend_64;
use thiserror::Error;

pub const MAX_BLOCK_INSTRUCTIONS: usize = 64;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum CompileError {
    #[error("Memory error while fetching instruction at 0x{0:016X}: {1}")]
    Memory(u64, MemoryError),
}

pub struct BlockCompiler;

impl BlockCompiler {
    /// Compiles a basic block starting from `start_pc` until a terminal control-flow instruction.
    pub fn compile(start_pc: u64, bus: &impl MemoryBus) -> Result<BasicBlock, CompileError> {
        let mut block = BasicBlock::new(start_pc);
        let mut curr_pc = start_pc;

        while block.instruction_count < MAX_BLOCK_INSTRUCTIONS {
            let raw_instr = match bus.read_u32(curr_pc) {
                Ok(w) => w,
                Err(e) => return Err(CompileError::Memory(curr_pc, e)),
            };

            let instr = Instruction(raw_instr);
            block.instruction_count += 1;
            block.guest_pc_end = curr_pc;

            // Check terminal branches first
            // 1. Unconditional Branch (B / BL): bits [26:30] == 00101
            if instr.bits(26, 30) == 0b00101 {
                let imm26 = instr.bits(0, 25) as u64;
                let offset = sign_extend_64(imm26, 26) * 4;
                let target_pc = (curr_pc as i64).wrapping_add(offset) as u64;
                let is_link = instr.bit(31);

                if is_link {
                    block.terminal = BlockTerminal::BranchCall {
                        target_pc,
                        return_pc: curr_pc.wrapping_add(4),
                    };
                } else {
                    block.terminal = BlockTerminal::BranchDirect { target_pc };
                }
                return Ok(block);
            }

            // 2. Conditional branch (B.cond): bits [24:31] == 01010100 and bit 4 == 0
            if instr.bits(24, 31) == 0b01010100 && !instr.bit(4) {
                let imm19 = instr.bits(5, 23) as u64;
                let offset = sign_extend_64(imm19, 19) * 4;
                let target_taken = (curr_pc as i64).wrapping_add(offset) as u64;
                let cond = instr.cond();

                block.terminal = BlockTerminal::BranchConditional {
                    cond,
                    target_taken,
                    target_fallthrough: curr_pc.wrapping_add(4),
                };
                return Ok(block);
            }

            // 3. Compare and branch (CBZ / CBNZ): bits [25:30] == 011010
            if instr.bits(25, 30) == 0b011010 {
                let is_64 = instr.is_64bit();
                let is_cbnz = instr.bit(24);
                let reg = instr.rd();
                let imm19 = instr.bits(5, 23) as u64;
                let offset = sign_extend_64(imm19, 19) * 4;
                let target_taken = (curr_pc as i64).wrapping_add(offset) as u64;

                block.terminal = BlockTerminal::CompareAndBranch {
                    reg,
                    is_64,
                    test_zero: !is_cbnz,
                    target_taken,
                    target_fallthrough: curr_pc.wrapping_add(4),
                };
                return Ok(block);
            }

            // 4. RET / BR: bits [10:31] == 1101011001011111000000 or bits [10:31] == 1101011000011111000000
            if instr.bits(10, 31) == 0b1101011001011111000000 || instr.bits(10, 31) == 0b1101011000011111000000 {
                let rn = instr.rn();
                block.terminal = BlockTerminal::BranchRegister { rn };
                return Ok(block);
            }

            // 5. SVC: bits [21:31] == 11010100000 and bits [0:4] == 00001
            if instr.bits(21, 31) == 0b11010100000 && instr.bits(0, 4) == 0b00001 {
                let imm16 = instr.bits(5, 20) as u16;
                block.terminal = BlockTerminal::Svc {
                    imm: imm16,
                    next_pc: curr_pc.wrapping_add(4),
                };
                return Ok(block);
            }

            // Translate linear instruction to MicroOp
            if let Some(op) = Self::translate_instruction(instr, curr_pc) {
                block.micro_ops.push(op);
            } else {
                // Non-translatable instruction: stop block and fallback to interpreter
                block.terminal = BlockTerminal::Fallback { target_pc: curr_pc };
                return Ok(block);
            }

            curr_pc = curr_pc.wrapping_add(4);
        }

        block.terminal = BlockTerminal::Fallthrough { next_pc: curr_pc };
        Ok(block)
    }

    /// Translates a single instruction word into a MicroOp, or None if fallback is needed.
    fn translate_instruction(instr: Instruction, curr_pc: u64) -> Option<MicroOp> {
        let op0 = instr.bits(25, 28);

        match op0 {
            // Data Processing -- Immediate: 100x
            0b1000 | 0b1001 => {
                let is_64 = instr.is_64bit();

                // PC-relative: ADR / ADRP (bits [24:25] == 00)
                if instr.bits(24, 25) == 0b00 {
                    let is_adrp = instr.bit(31);
                    let immlo = instr.bits(29, 30);
                    let immhi = instr.bits(5, 23);
                    let imm21 = (immhi << 2) | immlo;
                    let imm = sign_extend_64(imm21 as u64, 21);
                    let rd = instr.rd();

                    return if is_adrp {
                        Some(MicroOp::Adrp { rd, pc: curr_pc, imm: imm << 12 })
                    } else {
                        Some(MicroOp::Adr { rd, pc: curr_pc, imm })
                    };
                }

                // Add / Subtract (immediate): bits [23:25] == 010
                if instr.bits(23, 25) == 0b010 {
                    let is_sub = instr.bit(30);
                    let set_flags = instr.bit(29);
                    let shift = instr.bits(22, 23);
                    let imm12 = instr.bits(10, 21) as u64;
                    let imm = if shift == 1 { imm12 << 12 } else { imm12 };
                    let rd = instr.rd();
                    let rn = instr.rn();

                    return if is_sub {
                        Some(MicroOp::SubImm { rd, rn, imm, is_64, set_flags })
                    } else {
                        Some(MicroOp::AddImm { rd, rn, imm, is_64, set_flags })
                    };
                }

                // Move Wide (immediate): MOVZ / MOVN / MOVK (bits [23:25] == 101)
                if instr.bits(23, 25) == 0b101 {
                    let opc = instr.bits(29, 30);
                    let hw = instr.bits(21, 22);
                    let imm16 = instr.bits(5, 20) as u64;
                    let shift = hw * 16;
                    let val = imm16 << shift;
                    let rd = instr.rd();

                    return match opc {
                        0b00 => Some(MicroOp::MovWide { rd, imm: !val }), // MOVN
                        0b10 => Some(MicroOp::MovWide { rd, imm: val }),  // MOVZ
                        _ => None, // MOVK depends on prior register value, handled by fallback
                    };
                }

                None
            }

            // Data Processing -- Register: 0101, 1101
            0b0101 | 0b1101 => {
                let is_64 = instr.is_64bit();

                // Add / Subtract (shifted register): bits [24:28] == 01011
                if instr.bits(24, 28) == 0b01011 {
                    let is_sub = instr.bit(30);
                    let set_flags = instr.bit(29);
                    let rd = instr.rd();
                    let rn = instr.rn();
                    let rm = instr.rm();

                    // Only unshifted (shift == 0) supported directly in JIT MicroOp
                    let shift_type = instr.bits(22, 23);
                    let shift_amount = instr.bits(10, 15);
                    if shift_type == 0 && shift_amount == 0 {
                        return if is_sub {
                            Some(MicroOp::SubReg { rd, rn, rm, is_64, set_flags })
                        } else {
                            Some(MicroOp::AddReg { rd, rn, rm, is_64, set_flags })
                        };
                    }
                }

                // Logical (shifted register): bits [24:28] == 01010
                if instr.bits(24, 28) == 0b01010 {
                    let opc = instr.bits(29, 30);
                    let rd = instr.rd();
                    let rn = instr.rn();
                    let rm = instr.rm();

                    let shift_type = instr.bits(22, 23);
                    let shift_amount = instr.bits(10, 15);
                    if shift_type == 0 && shift_amount == 0 {
                        return match opc {
                            0b00 => Some(MicroOp::AndReg { rd, rn, rm, is_64, set_flags: false }), // AND
                            0b01 => Some(MicroOp::OrrReg { rd, rn, rm, is_64 }),                 // ORR
                            0b10 => Some(MicroOp::EorReg { rd, rn, rm, is_64 }),                 // EOR
                            0b11 => Some(MicroOp::AndReg { rd, rn, rm, is_64, set_flags: true }),  // ANDS
                            _ => None,
                        };
                    }
                }

                None
            }

            // Loads and Stores: x1x0
            0b0100 | 0b0110 | 0b1100 | 0b1110 => {
                // Single Register Load / Store (Unsigned Immediate Offset): bits [24:29] == 111001
                if instr.bits(24, 29) == 0b111001 && !instr.bit(26) {
                    let size_code = instr.bits(30, 31);
                    let is_load = instr.bits(22, 23) == 0b01;
                    let is_store = instr.bits(22, 23) == 0b00;
                    let imm12 = instr.bits(10, 21) as u64;
                    let rn = instr.rn();
                    let rt = instr.rd();

                    let (scale, size) = match size_code {
                        0b00 => (1, 1),
                        0b01 => (2, 2),
                        0b10 => (4, 4),
                        0b11 => (8, 8),
                        _ => (1, 1),
                    };
                    let offset = (imm12 * scale) as i64;

                    if is_load {
                        return Some(MicroOp::Ldr { rt, rn, offset, size });
                    } else if is_store {
                        return Some(MicroOp::Str { rt, rn, offset, size });
                    }
                }

                // Load / Store Register Pair (LDP / STP): bits [27:29] == 101
                if instr.bits(27, 29) == 0b101 && !instr.bit(26) {
                    let is_64 = instr.bits(30, 31) == 0b10;
                    let is_load = instr.bit(22);
                    let mode = instr.bits(23, 25);
                    let imm7 = instr.bits(15, 21) as u64;
                    let rn = instr.rn();
                    let rt1 = instr.rd();
                    let rt2 = instr.bits(10, 14) as usize;

                    // Signed offset mode (0b010)
                    if mode == 0b010 {
                        let scale = if is_64 { 8 } else { 4 };
                        let offset = sign_extend_64(imm7, 7) * scale;
                        return if is_load {
                            Some(MicroOp::Ldp { rt1, rt2, rn, offset, is_64 })
                        } else {
                            Some(MicroOp::Stp { rt1, rt2, rn, offset, is_64 })
                        };
                    }
                }

                None
            }

            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::jit::block::BlockOutcome;
    use crate::cpu::registers::RegisterFile;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_jit_compile_and_execute_arithmetic_sequence() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x1000);
        let mut regs = RegisterFile::default();

        // 1. MOVZ X0, #42        (0xD2800540)
        // 2. ADD X1, X0, #8      (0x91002001)
        // 3. SUB X2, X1, #10     (0xD1002822)
        // 4. SVC #7              (0xD40000E1)
        let code = [
            0xD2800540u32, // movz x0, #42
            0x91002001u32, // add x1, x0, #8
            0xD1002822u32, // sub x2, x1, #10
            0xD40000E1u32, // svc #7
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        let mut block = BlockCompiler::compile(0x8000_0000, &mem).unwrap();
        assert_eq!(block.instruction_count, 4);
        assert_eq!(block.micro_ops.len(), 3); // 3 ALU ops + 1 terminal SVC

        let outcome = block.execute(&mut regs, &mut mem).unwrap();
        assert_eq!(outcome, BlockOutcome::Svc(7, 0x8000_0010));

        assert_eq!(regs.get_x(0), 42);
        assert_eq!(regs.get_x(1), 50);
        assert_eq!(regs.get_x(2), 40);
        assert_eq!(regs.pc, 0x8000_0010);
    }

    #[test]
    fn test_jit_compile_conditional_branch() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x1000);

        // 1. CMP X0, #42 (0xF100A81F)
        // 2. B.EQ +8     (0x54000040)
        let code = [
            0xF100A81Fu32,
            0x54000040u32,
        ];

        for (i, &instr) in code.iter().enumerate() {
            mem.write_u32(0x8000_0000 + (i as u64) * 4, instr).unwrap();
        }

        let block = BlockCompiler::compile(0x8000_0000, &mem).unwrap();
        assert_eq!(block.instruction_count, 2);

        match block.terminal {
            BlockTerminal::BranchConditional { cond, target_taken, target_fallthrough } => {
                assert_eq!(cond, 0); // EQ
                assert_eq!(target_taken, 0x8000_000C);
                assert_eq!(target_fallthrough, 0x8000_0008);
            }
            other => panic!("Expected BranchConditional, got {:?}", other),
        }
    }
}
