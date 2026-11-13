//! Basic Block Intermediate Representation (IR) and Micro-Op Execution.
//!
//! Pre-decodes AArch64 machine instructions into atomic MicroOps to eliminate
//! decoding, bitfield extraction, and condition inspection overhead during execution.

use crate::cpu::registers::{PState, RegisterFile};
use crate::memory::bus::{MemoryBus, MemoryError};

/// Low-overhead, pre-decoded intermediate instruction representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicroOp {
    Nop,
    MovWide {
        rd: usize,
        imm: u64,
    },
    AddImm {
        rd: usize,
        rn: usize,
        imm: u64,
        is_64: bool,
        set_flags: bool,
    },
    SubImm {
        rd: usize,
        rn: usize,
        imm: u64,
        is_64: bool,
        set_flags: bool,
    },
    AddReg {
        rd: usize,
        rn: usize,
        rm: usize,
        is_64: bool,
        set_flags: bool,
    },
    SubReg {
        rd: usize,
        rn: usize,
        rm: usize,
        is_64: bool,
        set_flags: bool,
    },
    AndReg {
        rd: usize,
        rn: usize,
        rm: usize,
        is_64: bool,
        set_flags: bool,
    },
    OrrReg {
        rd: usize,
        rn: usize,
        rm: usize,
        is_64: bool,
    },
    EorReg {
        rd: usize,
        rn: usize,
        rm: usize,
        is_64: bool,
    },
    Adr {
        rd: usize,
        pc: u64,
        imm: i64,
    },
    Adrp {
        rd: usize,
        pc: u64,
        imm: i64,
    },
    Ldr {
        rt: usize,
        rn: usize,
        offset: i64,
        size: usize,
    },
    Str {
        rt: usize,
        rn: usize,
        offset: i64,
        size: usize,
    },
    Ldp {
        rt1: usize,
        rt2: usize,
        rn: usize,
        offset: i64,
        is_64: bool,
    },
    Stp {
        rt1: usize,
        rt2: usize,
        rn: usize,
        offset: i64,
        is_64: bool,
    },
}

/// Control-flow exit condition for a translated basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockTerminal {
    /// Static direct branch (e.g. `B target`).
    BranchDirect { target_pc: u64 },
    /// Conditional branch (e.g. `B.cond target`).
    BranchConditional {
        cond: u32,
        target_taken: u64,
        target_fallthrough: u64,
    },
    /// Subroutine call with return link (e.g. `BL target`).
    BranchCall { target_pc: u64, return_pc: u64 },
    /// Indirect branch via register (e.g. `RET`, `BR Rn`).
    BranchRegister { rn: usize },
    /// Compare and branch if zero/non-zero (e.g. `CBZ`, `CBNZ`).
    CompareAndBranch {
        reg: usize,
        is_64: bool,
        test_zero: bool,
        target_taken: u64,
        target_fallthrough: u64,
    },
    /// Supervisor Call (SVC).
    Svc { imm: u16, next_pc: u64 },
    /// Reached block size limit without branch; seamless fallthrough.
    Fallthrough { next_pc: u64 },
    /// Unimplemented or complex instruction; fall back to CPU interpreter.
    Fallback { target_pc: u64 },
}

/// Result of executing a compiled basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockOutcome {
    /// Normal branch to next guest PC.
    Jump(u64),
    /// Supervisor Call invoked with immediate and next PC.
    Svc(u16, u64),
    /// Fallback to interpreter at designated PC.
    Fallback(u64),
}

/// A translated sequence of linear instructions terminating in a control-flow transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub guest_pc_start: u64,
    pub guest_pc_end: u64,
    pub instruction_count: usize,
    pub micro_ops: Vec<MicroOp>,
    pub terminal: BlockTerminal,
    pub execution_count: u64,
}

impl BasicBlock {
    pub fn new(guest_pc_start: u64) -> Self {
        Self {
            guest_pc_start,
            guest_pc_end: guest_pc_start,
            instruction_count: 0,
            micro_ops: Vec::new(),
            terminal: BlockTerminal::Fallthrough { next_pc: guest_pc_start },
            execution_count: 0,
        }
    }

    /// Evaluates condition codes (bits [3:0]) against current PSTATE.NZCV flags.
    #[inline(always)]
    fn evaluate_condition(pstate: &PState, cond: u32) -> bool {
        match cond {
            0b0000 => pstate.z,
            0b0001 => !pstate.z,
            0b0010 => pstate.c,
            0b0011 => !pstate.c,
            0b0100 => pstate.n,
            0b0101 => !pstate.n,
            0b0110 => pstate.v,
            0b0111 => !pstate.v,
            0b1000 => pstate.c && !pstate.z,
            0b1001 => !pstate.c || pstate.z,
            0b1010 => pstate.n == pstate.v,
            0b1011 => pstate.n != pstate.v,
            0b1100 => !pstate.z && (pstate.n == pstate.v),
            0b1101 => pstate.z || (pstate.n != pstate.v),
            0b1110 | 0b1111 => true,
            _ => unreachable!(),
        }
    }

    /// Executes all micro-ops sequentially without instruction decoding overhead,
    /// then resolves the terminal condition to return the next guest PC or SVC.
    pub fn execute(&mut self, regs: &mut RegisterFile, bus: &mut impl MemoryBus) -> Result<BlockOutcome, MemoryError> {
        self.execution_count = self.execution_count.wrapping_add(1);

        for op in &self.micro_ops {
            match *op {
                MicroOp::Nop => {}

                MicroOp::MovWide { rd, imm } => {
                    regs.set_x(rd, imm);
                }

                MicroOp::AddImm { rd, rn, imm, is_64, set_flags } => {
                    let op1 = regs.get_reg_or_sp(rn);
                    if is_64 {
                        let res = op1.wrapping_add(imm);
                        if set_flags {
                            let (_c_res, c_out) = op1.overflowing_add(imm);
                            let signed_op1 = op1 as i64;
                            let signed_op2 = imm as i64;
                            let (_, v_out) = signed_op1.overflowing_add(signed_op2);
                            regs.pstate.n = (res as i64) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = c_out;
                            regs.pstate.v = v_out;
                        }
                        regs.set_reg_or_sp(rd, res);
                    } else {
                        let op1_32 = op1 as u32;
                        let imm_32 = imm as u32;
                        let res = op1_32.wrapping_add(imm_32);
                        if set_flags {
                            let (_, c_out) = op1_32.overflowing_add(imm_32);
                            let (_, v_out) = (op1_32 as i32).overflowing_add(imm_32 as i32);
                            regs.pstate.n = (res as i32) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = c_out;
                            regs.pstate.v = v_out;
                        }
                        regs.set_w(rd, res);
                    }
                }

                MicroOp::SubImm { rd, rn, imm, is_64, set_flags } => {
                    let op1 = regs.get_reg_or_sp(rn);
                    if is_64 {
                        let res = op1.wrapping_sub(imm);
                        if set_flags {
                            let (_c_res, c_borrow) = op1.overflowing_sub(imm);
                            let (_, v_out) = (op1 as i64).overflowing_sub(imm as i64);
                            regs.pstate.n = (res as i64) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = !c_borrow;
                            regs.pstate.v = v_out;
                        }
                        regs.set_reg_or_sp(rd, res);
                    } else {
                        let op1_32 = op1 as u32;
                        let imm_32 = imm as u32;
                        let res = op1_32.wrapping_sub(imm_32);
                        if set_flags {
                            let (_, c_borrow) = op1_32.overflowing_sub(imm_32);
                            let (_, v_out) = (op1_32 as i32).overflowing_sub(imm_32 as i32);
                            regs.pstate.n = (res as i32) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = !c_borrow;
                            regs.pstate.v = v_out;
                        }
                        regs.set_w(rd, res);
                    }
                }

                MicroOp::AddReg { rd, rn, rm, is_64, set_flags } => {
                    let op1 = regs.get_x(rn);
                    let op2 = regs.get_x(rm);
                    if is_64 {
                        let res = op1.wrapping_add(op2);
                        if set_flags {
                            let (_, c_out) = op1.overflowing_add(op2);
                            let (_, v_out) = (op1 as i64).overflowing_add(op2 as i64);
                            regs.pstate.n = (res as i64) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = c_out;
                            regs.pstate.v = v_out;
                        }
                        regs.set_x(rd, res);
                    } else {
                        let op1_32 = op1 as u32;
                        let op2_32 = op2 as u32;
                        let res = op1_32.wrapping_add(op2_32);
                        if set_flags {
                            let (_, c_out) = op1_32.overflowing_add(op2_32);
                            let (_, v_out) = (op1_32 as i32).overflowing_add(op2_32 as i32);
                            regs.pstate.n = (res as i32) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = c_out;
                            regs.pstate.v = v_out;
                        }
                        regs.set_w(rd, res);
                    }
                }

                MicroOp::SubReg { rd, rn, rm, is_64, set_flags } => {
                    let op1 = regs.get_x(rn);
                    let op2 = regs.get_x(rm);
                    if is_64 {
                        let res = op1.wrapping_sub(op2);
                        if set_flags {
                            let (_, c_borrow) = op1.overflowing_sub(op2);
                            let (_, v_out) = (op1 as i64).overflowing_sub(op2 as i64);
                            regs.pstate.n = (res as i64) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = !c_borrow;
                            regs.pstate.v = v_out;
                        }
                        regs.set_x(rd, res);
                    } else {
                        let op1_32 = op1 as u32;
                        let op2_32 = op2 as u32;
                        let res = op1_32.wrapping_sub(op2_32);
                        if set_flags {
                            let (_, c_borrow) = op1_32.overflowing_sub(op2_32);
                            let (_, v_out) = (op1_32 as i32).overflowing_sub(op2_32 as i32);
                            regs.pstate.n = (res as i32) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = !c_borrow;
                            regs.pstate.v = v_out;
                        }
                        regs.set_w(rd, res);
                    }
                }

                MicroOp::AndReg { rd, rn, rm, is_64, set_flags } => {
                    let op1 = regs.get_x(rn);
                    let op2 = regs.get_x(rm);
                    let res = op1 & op2;
                    if is_64 {
                        if set_flags {
                            regs.pstate.n = (res as i64) < 0;
                            regs.pstate.z = res == 0;
                            regs.pstate.c = false;
                            regs.pstate.v = false;
                        }
                        regs.set_x(rd, res);
                    } else {
                        let res32 = res as u32;
                        if set_flags {
                            regs.pstate.n = (res32 as i32) < 0;
                            regs.pstate.z = res32 == 0;
                            regs.pstate.c = false;
                            regs.pstate.v = false;
                        }
                        regs.set_w(rd, res32);
                    }
                }

                MicroOp::OrrReg { rd, rn, rm, is_64 } => {
                    let res = regs.get_x(rn) | regs.get_x(rm);
                    if is_64 {
                        regs.set_x(rd, res);
                    } else {
                        regs.set_w(rd, res as u32);
                    }
                }

                MicroOp::EorReg { rd, rn, rm, is_64 } => {
                    let res = regs.get_x(rn) ^ regs.get_x(rm);
                    if is_64 {
                        regs.set_x(rd, res);
                    } else {
                        regs.set_w(rd, res as u32);
                    }
                }

                MicroOp::Adr { rd, pc, imm } => {
                    let target = (pc as i64).wrapping_add(imm) as u64;
                    regs.set_x(rd, target);
                }

                MicroOp::Adrp { rd, pc, imm } => {
                    let page_base = pc & !0xFFF;
                    let target = (page_base as i64).wrapping_add(imm) as u64;
                    regs.set_x(rd, target);
                }

                MicroOp::Ldr { rt, rn, offset, size } => {
                    let base = regs.get_reg_or_sp(rn);
                    let addr = (base as i64).wrapping_add(offset) as u64;
                    match size {
                        1 => {
                            let val = bus.read_u8(addr)?;
                            regs.set_w(rt, val as u32);
                        }
                        2 => {
                            let val = bus.read_u16(addr)?;
                            regs.set_w(rt, val as u32);
                        }
                        4 => {
                            let val = bus.read_u32(addr)?;
                            regs.set_w(rt, val);
                        }
                        8 => {
                            let val = bus.read_u64(addr)?;
                            regs.set_x(rt, val);
                        }
                        _ => {}
                    }
                }

                MicroOp::Str { rt, rn, offset, size } => {
                    let base = regs.get_reg_or_sp(rn);
                    let addr = (base as i64).wrapping_add(offset) as u64;
                    match size {
                        1 => bus.write_u8(addr, regs.get_w(rt) as u8)?,
                        2 => bus.write_u16(addr, regs.get_w(rt) as u16)?,
                        4 => bus.write_u32(addr, regs.get_w(rt))?,
                        8 => bus.write_u64(addr, regs.get_x(rt))?,
                        _ => {}
                    }
                }

                MicroOp::Ldp { rt1, rt2, rn, offset, is_64 } => {
                    let base = regs.get_reg_or_sp(rn);
                    let addr = (base as i64).wrapping_add(offset) as u64;
                    if is_64 {
                        let v1 = bus.read_u64(addr)?;
                        let v2 = bus.read_u64(addr + 8)?;
                        regs.set_x(rt1, v1);
                        regs.set_x(rt2, v2);
                    } else {
                        let v1 = bus.read_u32(addr)?;
                        let v2 = bus.read_u32(addr + 4)?;
                        regs.set_w(rt1, v1);
                        regs.set_w(rt2, v2);
                    }
                }

                MicroOp::Stp { rt1, rt2, rn, offset, is_64 } => {
                    let base = regs.get_reg_or_sp(rn);
                    let addr = (base as i64).wrapping_add(offset) as u64;
                    if is_64 {
                        bus.write_u64(addr, regs.get_x(rt1))?;
                        bus.write_u64(addr + 8, regs.get_x(rt2))?;
                    } else {
                        bus.write_u32(addr, regs.get_w(rt1))?;
                        bus.write_u32(addr + 4, regs.get_w(rt2))?;
                    }
                }
            }
        }

        // Resolve terminal condition
        match self.terminal {
            BlockTerminal::BranchDirect { target_pc } => {
                regs.pc = target_pc;
                Ok(BlockOutcome::Jump(target_pc))
            }

            BlockTerminal::BranchConditional { cond, target_taken, target_fallthrough } => {
                let taken = Self::evaluate_condition(&regs.pstate, cond);
                let target = if taken { target_taken } else { target_fallthrough };
                regs.pc = target;
                Ok(BlockOutcome::Jump(target))
            }

            BlockTerminal::BranchCall { target_pc, return_pc } => {
                regs.set_x(30, return_pc); // Link register LR = X30
                regs.pc = target_pc;
                Ok(BlockOutcome::Jump(target_pc))
            }

            BlockTerminal::BranchRegister { rn } => {
                let target = regs.get_x(rn);
                regs.pc = target;
                Ok(BlockOutcome::Jump(target))
            }

            BlockTerminal::CompareAndBranch { reg, is_64, test_zero, target_taken, target_fallthrough } => {
                let val = if is_64 { regs.get_x(reg) } else { regs.get_w(reg) as u64 };
                let is_zero = val == 0;
                let taken = if test_zero { is_zero } else { !is_zero };
                let target = if taken { target_taken } else { target_fallthrough };
                regs.pc = target;
                Ok(BlockOutcome::Jump(target))
            }

            BlockTerminal::Svc { imm, next_pc } => {
                regs.pc = next_pc;
                Ok(BlockOutcome::Svc(imm, next_pc))
            }

            BlockTerminal::Fallthrough { next_pc } => {
                regs.pc = next_pc;
                Ok(BlockOutcome::Jump(next_pc))
            }

            BlockTerminal::Fallback { target_pc } => {
                regs.pc = target_pc;
                Ok(BlockOutcome::Fallback(target_pc))
            }
        }
    }
}
