//! Maxwell 64-bit SASS Instruction Representation and Operand Extraction.

/// Maxwell Predicate condition.
/// Index 7 (PT) is the hardware constant TRUE predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Predicate {
    pub index: u8,
    pub negated: bool,
}

impl Predicate {
    pub const TRUE: Self = Self {
        index: 7,
        negated: false,
    };

    pub fn new(index: u8, negated: bool) -> Self {
        Self { index, negated }
    }

    /// Extracts predicate from instruction word bits [19:16].
    pub fn from_raw(raw: u64) -> Self {
        let val = ((raw >> 16) & 0xF) as u8;
        Self {
            index: val & 0x7,
            negated: (val & 0x8) != 0,
        }
    }

    pub fn is_unconditional(&self) -> bool {
        self.index == 7 && !self.negated
    }
}

/// 64-bit Maxwell SASS Instruction Word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderInstruction {
    pub raw: u64,
}

impl ShaderInstruction {
    pub const RZ: u8 = 255; // Register Zero / Discard

    pub fn new(raw: u64) -> Self {
        Self { raw }
    }

    /// Execution predicate (bits [19:16]).
    pub fn predicate(&self) -> Predicate {
        Predicate::from_raw(self.raw)
    }

    /// Destination register Rd (bits [7:0]).
    pub fn rd(&self) -> u8 {
        (self.raw & 0xFF) as u8
    }

    /// Source register Ra (bits [15:8]).
    pub fn ra(&self) -> u8 {
        ((self.raw >> 8) & 0xFF) as u8
    }

    /// Source register Rb (bits [27:20]).
    pub fn rb(&self) -> u8 {
        ((self.raw >> 20) & 0xFF) as u8
    }

    /// Source register Rc for 3-operand ops like FFMA (bits [47:40]).
    pub fn rc(&self) -> u8 {
        ((self.raw >> 40) & 0xFF) as u8
    }

    /// Constant buffer bank (bits [27:24] or [31:26]).
    pub fn cbuf_bank(&self) -> u16 {
        ((self.raw >> 24) & 0x1F) as u16
    }

    /// Constant buffer byte offset (bits [43:28] or [39:24]).
    pub fn cbuf_offset(&self) -> u32 {
        (((self.raw >> 28) & 0xFFFF) as u32) * 4
    }

    /// Immediate 20-bit float (bits [39:20]).
    pub fn imm20_float(&self) -> f32 {
        let raw20 = ((self.raw >> 20) & 0xFFFFF) as u32;
        // Sign: bit 19, Exponent: bits [18:11], Mantissa: bits [10:0]
        let sign = (raw20 >> 19) & 1;
        let exp = (raw20 >> 11) & 0xFF;
        let mant = raw20 & 0x7FF;
        let f32_bits = (sign << 31) | (exp << 23) | (mant << 12);
        f32::from_bits(f32_bits)
    }

    /// Immediate 32-bit integer (bits [51:20]).
    pub fn imm32(&self) -> u32 {
        ((self.raw >> 20) & 0xFFFF_FFFF) as u32
    }

    /// Attribute index for ALD / AST (bits [27:20]).
    pub fn attribute_offset(&self) -> u32 {
        ((self.raw >> 20) & 0xFF) as u32 * 4
    }

    /// Branch relative PC target offset (signed 24-bit, bits [43:20]).
    pub fn branch_offset(&self) -> i32 {
        let raw24 = ((self.raw >> 20) & 0x00FF_FFFF) as u32;
        if raw24 & (1 << 23) != 0 {
            (raw24 | 0xFF00_0000) as i32
        } else {
            raw24 as i32
        }
    }

    /// Texture / Sampler unit identifier (bits [35:28]).
    pub fn sampler_index(&self) -> u32 {
        ((self.raw >> 28) & 0xFF) as u32
    }

    /// Top 16 bits opcode class (bits [63:48]).
    pub fn opcode_class(&self) -> u16 {
        (self.raw >> 48) as u16
    }
}
