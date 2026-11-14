//! Maxwell GPU SASS Opcode Enumeration and Decoding.

use super::instruction::ShaderInstruction;

/// Decoded Maxwell instruction opcodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaxwellOpcode {
    Nop,
    // Floating-Point Arithmetic
    FAdd,
    FMul,
    FFma,
    FMinMax,
    FCmp,
    Mufu,
    // Integer & Logic
    IAdd,
    IAdd3,
    ISub,
    IMul,
    ISetP,
    FSetP,
    // Memory & Uniforms
    Ldc,
    Ldg,
    Stg,
    // Attributes (Vertex input / Fragment output)
    Ald,
    Ast,
    // Texture Units
    Tex,
    Tld,
    // Flow Control
    Bra,
    Ssy,
    Sync,
    Kil,
    Exit,
    // Fallback for unhandled/vendor ops
    Unknown(u16),
}

/// Decodes the 64-bit instruction word into a MaxwellOpcode.
pub fn decode_opcode(insn: &ShaderInstruction) -> MaxwellOpcode {
    let raw = insn.raw;
    if raw == 0 {
        return MaxwellOpcode::Nop;
    }

    let top16 = (raw >> 48) as u16;

    // Pattern matching on upper 16-bit opcode class
    match top16 {
        0x50B0 => MaxwellOpcode::Nop,

        // Floating-point Add: FADD (reg: 0x5C58, cbuf: 0x4C58, imm: 0x3858)
        0x5C58 | 0x4C58 | 0x3858 => MaxwellOpcode::FAdd,

        // Floating-point Multiply: FMUL (reg: 0x5C68, cbuf: 0x4C68, imm: 0x3868)
        0x5C68 | 0x4C68 | 0x3868 => MaxwellOpcode::FMul,

        // Fused Multiply-Add: FFMA (reg: 0x5980, cbuf: 0x4980, imm: 0x3980, rc: 0x5880)
        0x5980 | 0x4980 | 0x3980 | 0x5880 => MaxwellOpcode::FFma,

        // Float Min/Max: FMNMX
        0x5C60 | 0x4C60 => MaxwellOpcode::FMinMax,

        // Float Compare: FCMP
        0x5C70 => MaxwellOpcode::FCmp,

        // Multi-function Unit: MUFU (sin, cos, rcp, rsqrt)
        0x5080 => MaxwellOpcode::Mufu,

        // Integer operations
        0x5C10 | 0x4C10 => MaxwellOpcode::IAdd3,
        0x5C08 | 0x4C08 => MaxwellOpcode::IAdd,
        0x5B50 | 0x4B50 => MaxwellOpcode::ISetP,
        0x5B60 | 0x4B60 => MaxwellOpcode::FSetP,

        // Load Constant Buffer: LDC
        0xEF90 => MaxwellOpcode::Ldc,

        // Load / Store Global: LDG / STG
        0x3810 | 0xEEDB => MaxwellOpcode::Ldg,
        0x3820 | 0xEEDE => MaxwellOpcode::Stg,

        // Attribute Load / Store: ALD / AST
        0xEFD8 => MaxwellOpcode::Ald,
        0xEFF0 => MaxwellOpcode::Ast,

        // Texture Sampling / Fetch: TEX / TLD
        0xB000 | 0xDE00 => MaxwellOpcode::Tex,
        0xB020 | 0xDE20 => MaxwellOpcode::Tld,

        // Control flow: BRA, SSY, SYNC, KIL, EXIT
        0xE240 => MaxwellOpcode::Bra,
        0xE250 => MaxwellOpcode::Ssy,
        0xF0F0 => MaxwellOpcode::Sync,
        0xE330 => MaxwellOpcode::Kil,
        0xE300 | 0x9480 => MaxwellOpcode::Exit,

        other => MaxwellOpcode::Unknown(other),
    }
}
