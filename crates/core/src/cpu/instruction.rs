//! AArch64 32-bit instruction word abstraction and bitfield extraction.

use common::{extract_bits_u32, test_bit_u32};

/// Represents a raw 32-bit AArch64 instruction word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction(pub u32);

impl Instruction {
    /// Extracts a contiguous range of bits `[msb:lsb]` inclusive.
    #[inline(always)]
    pub const fn bits(self, lsb: u32, msb: u32) -> u32 {
        extract_bits_u32(self.0, lsb, msb)
    }

    /// Tests a single bit at `bit_idx`.
    #[inline(always)]
    pub const fn bit(self, bit_idx: u32) -> bool {
        test_bit_u32(self.0, bit_idx)
    }

    // Common AArch64 Register and Operand Fields

    /// Destination register `Rd` (bits 4:0).
    #[inline(always)]
    pub const fn rd(self) -> usize {
        self.bits(0, 4) as usize
    }

    /// First source register `Rn` (bits 9:5).
    #[inline(always)]
    pub const fn rn(self) -> usize {
        self.bits(5, 9) as usize
    }

    /// Second source register `Rm` (bits 20:16).
    #[inline(always)]
    pub const fn rm(self) -> usize {
        self.bits(16, 20) as usize
    }

    /// Third source register `Ra` (bits 14:10).
    #[inline(always)]
    pub const fn ra(self) -> usize {
        self.bits(10, 14) as usize
    }

    /// Standard 64-bit (`sf` = 1) vs 32-bit (`sf` = 0) operation flag (bit 31).
    #[inline(always)]
    pub const fn is_64bit(self) -> bool {
        self.bit(31)
    }

    /// Condition code for conditional branches (bits 3:0).
    #[inline(always)]
    pub const fn cond(self) -> u32 {
        self.bits(0, 3)
    }
}
