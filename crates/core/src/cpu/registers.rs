//! AArch64 Processor State and Register File Layout.

/// Processor state flags (PSTATE.NZCV).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PState {
    /// Negative condition flag.
    pub n: bool,
    /// Zero condition flag.
    pub z: bool,
    /// Carry condition flag.
    pub c: bool,
    /// Overflow condition flag.
    pub v: bool,
}

impl PState {
    /// Encodes NZCV into the standard 32-bit format (bits 31:28).
    pub fn to_raw(self) -> u32 {
        let n_bit = (self.n as u32) << 31;
        let z_bit = (self.z as u32) << 30;
        let c_bit = (self.c as u32) << 29;
        let v_bit = (self.v as u32) << 28;
        n_bit | z_bit | c_bit | v_bit
    }

    /// Decodes NZCV from standard raw bits.
    pub fn from_raw(raw: u32) -> Self {
        Self {
            n: (raw & (1 << 31)) != 0,
            z: (raw & (1 << 30)) != 0,
            c: (raw & (1 << 29)) != 0,
            v: (raw & (1 << 28)) != 0,
        }
    }
}

/// AArch64 architectural register file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterFile {
    /// General purpose registers X0 - X30 (64-bit each).
    pub x: [u64; 31],
    /// Dedicated Stack Pointer (SP).
    pub sp: u64,
    /// Program Counter (PC).
    pub pc: u64,
    /// Condition Flags (NZCV).
    pub pstate: PState,
    /// 32 128-bit Vector / Floating Point registers (V0 - V31).
    pub v: [[u8; 16]; 32],
    /// Thread Local Storage register for userland EL0 (TPIDR_EL0).
    pub tpidr_el0: u64,
}

impl Default for RegisterFile {
    fn default() -> Self {
        Self {
            x: [0u64; 31],
            sp: 0,
            pc: 0,
            pstate: PState::default(),
            v: [[0u8; 16]; 32],
            tpidr_el0: 0,
        }
    }
}

impl RegisterFile {
    pub const ZR_INDEX: usize = 31;

    /// Reads a 64-bit general-purpose register X0-X30.
    /// Reading register 31 (XZR) always yields 0.
    #[inline(always)]
    pub fn get_x(&self, reg: usize) -> u64 {
        if reg == Self::ZR_INDEX {
            0
        } else {
            self.x[reg]
        }
    }

    /// Writes a 64-bit general-purpose register X0-X30.
    /// Writing to register 31 (XZR) is a no-op.
    #[inline(always)]
    pub fn set_x(&mut self, reg: usize, val: u64) {
        if reg != Self::ZR_INDEX {
            self.x[reg] = val;
        }
    }

    /// Reads a 32-bit view W0-W30 of a general-purpose register.
    /// Reading register 31 (WZR) yields 0.
    #[inline(always)]
    pub fn get_w(&self, reg: usize) -> u32 {
        self.get_x(reg) as u32
    }

    /// Writes a 32-bit value to W0-W30.
    /// In ARM64, writing to a 32-bit W register unconditionally zeroes the upper 32 bits of X.
    /// Writing to register 31 (WZR) is a no-op.
    #[inline(always)]
    pub fn set_w(&mut self, reg: usize, val: u32) {
        if reg != Self::ZR_INDEX {
            self.x[reg] = val as u64;
        }
    }

    /// Reads register 0-30 or SP if reg is 31 (for instructions using SP instead of ZR).
    #[inline(always)]
    pub fn get_reg_or_sp(&self, reg: usize) -> u64 {
        if reg == Self::ZR_INDEX {
            self.sp
        } else {
            self.x[reg]
        }
    }

    /// Writes register 0-30 or SP if reg is 31.
    #[inline(always)]
    pub fn set_reg_or_sp(&mut self, reg: usize, val: u64) {
        if reg == Self::ZR_INDEX {
            self.sp = val;
        } else {
            self.x[reg] = val;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_register_behavior() {
        let mut rf = RegisterFile::default();
        
        // Reading ZR initially returns 0
        assert_eq!(rf.get_x(31), 0);
        assert_eq!(rf.get_w(31), 0);

        // Attempting to write to ZR is discarded
        rf.set_x(31, 0xDEAD_BEEF_CAFE_BABE);
        assert_eq!(rf.get_x(31), 0);

        rf.set_w(31, 0x1234_5678);
        assert_eq!(rf.get_w(31), 0);
    }

    #[test]
    fn test_w_register_zero_extension() {
        let mut rf = RegisterFile::default();

        // Write a 64-bit value to X1
        rf.set_x(1, 0xFFFF_FFFF_FFFF_FFFF);
        assert_eq!(rf.get_x(1), 0xFFFF_FFFF_FFFF_FFFF);

        // Write a 32-bit value to W1 - MUST zero out the upper 32 bits
        rf.set_w(1, 0x1234_5678);
        assert_eq!(rf.get_x(1), 0x0000_0000_1234_5678);
        assert_eq!(rf.get_w(1), 0x1234_5678);
    }

    #[test]
    fn test_reg_or_sp() {
        let mut rf = RegisterFile::default();
        rf.set_reg_or_sp(31, 0x7FFF_0000);
        assert_eq!(rf.sp, 0x7FFF_0000);
        assert_eq!(rf.get_reg_or_sp(31), 0x7FFF_0000);
        // XZR should still be 0
        assert_eq!(rf.get_x(31), 0);
    }

    #[test]
    fn test_pstate_raw() {
        let pstate = PState {
            n: true,
            z: false,
            c: true,
            v: false,
        };
        let raw = pstate.to_raw();
        assert_eq!(raw, 0xA000_0000);
        let decoded = PState::from_raw(raw);
        assert_eq!(pstate, decoded);
    }
}
