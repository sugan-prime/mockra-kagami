//! Maxwell SASS Bytecode Decompiler to Intermediate Representation and Shading Languages (WGSL / GLSL).

use super::instruction::ShaderInstruction;
use super::ir::{IrOp, ShaderBasicBlock, ShaderProgram, SourceOperand};
use super::opcode::{decode_opcode, MaxwellOpcode};
use crate::gpu::shader_cache::ShaderStage;

pub struct ShaderDecompiler;

impl ShaderDecompiler {
    /// Decompiles raw Maxwell SASS bytecode into a structured `ShaderProgram`.
    pub fn decompile(bytecode: &[u8], stage: ShaderStage) -> ShaderProgram {
        let mut program = ShaderProgram::new(stage);
        let mut current_block = ShaderBasicBlock::new(0);

        let mut pc: usize = 0;
        let total_bytes = bytecode.len();

        while pc + 8 <= total_bytes {
            let insn_raw = u64::from_le_bytes(bytecode[pc..pc + 8].try_into().unwrap());
            let insn = ShaderInstruction::new(insn_raw);
            let opcode = decode_opcode(&insn);

            let ir_op = match opcode {
                MaxwellOpcode::Nop => IrOp::Nop,

                MaxwellOpcode::FAdd => {
                    let dst = insn.rd();
                    let src_a = SourceOperand::Register(insn.ra());
                    let src_b = if insn.opcode_class() == 0x4C58 {
                        SourceOperand::ConstantBuffer {
                            bank: insn.cbuf_bank(),
                            offset: insn.cbuf_offset(),
                        }
                    } else if insn.opcode_class() == 0x3858 {
                        SourceOperand::ImmediateFloat(insn.imm20_float())
                    } else {
                        SourceOperand::Register(insn.rb())
                    };
                    IrOp::FAdd { dst, src_a, src_b }
                }

                MaxwellOpcode::FMul => {
                    let dst = insn.rd();
                    let src_a = SourceOperand::Register(insn.ra());
                    let src_b = if insn.opcode_class() == 0x4C68 {
                        SourceOperand::ConstantBuffer {
                            bank: insn.cbuf_bank(),
                            offset: insn.cbuf_offset(),
                        }
                    } else if insn.opcode_class() == 0x3868 {
                        SourceOperand::ImmediateFloat(insn.imm20_float())
                    } else {
                        SourceOperand::Register(insn.rb())
                    };
                    IrOp::FMul { dst, src_a, src_b }
                }

                MaxwellOpcode::FFma => {
                    let dst = insn.rd();
                    let src_a = SourceOperand::Register(insn.ra());
                    let src_b = if insn.opcode_class() == 0x4980 {
                        SourceOperand::ConstantBuffer {
                            bank: insn.cbuf_bank(),
                            offset: insn.cbuf_offset(),
                        }
                    } else {
                        SourceOperand::Register(insn.rb())
                    };
                    let src_c = SourceOperand::Register(insn.rc());
                    IrOp::FFma { dst, src_a, src_b, src_c }
                }

                MaxwellOpcode::Ald => {
                    let dst = insn.rd();
                    let attr_offset = insn.attribute_offset();
                    IrOp::LoadAttribute { dst, attr_offset }
                }

                MaxwellOpcode::Ast => {
                    let attr_offset = insn.attribute_offset();
                    let src = SourceOperand::Register(insn.ra());
                    IrOp::StoreAttribute { attr_offset, src }
                }

                MaxwellOpcode::Ldc => {
                    let dst = insn.rd();
                    let bank = insn.cbuf_bank();
                    let offset = insn.cbuf_offset();
                    IrOp::LoadConstant { dst, bank, offset }
                }

                MaxwellOpcode::Tex => {
                    let dst = insn.rd();
                    let sampler_index = insn.sampler_index();
                    let coord_u = SourceOperand::Register(insn.ra());
                    let coord_v = SourceOperand::Register(insn.rb());
                    IrOp::TextureSample {
                        dst_rgba: [dst, dst.wrapping_add(1), dst.wrapping_add(2), dst.wrapping_add(3)],
                        sampler_index,
                        coord_u,
                        coord_v,
                    }
                }

                MaxwellOpcode::Bra => {
                    let offset = insn.branch_offset();
                    let target_pc = ((pc as i64) + (offset as i64) + 8).max(0) as u64;
                    let pred = insn.predicate();
                    if pred.is_unconditional() {
                        IrOp::Branch { target_pc }
                    } else {
                        IrOp::BranchCond { pred, target_pc }
                    }
                }

                MaxwellOpcode::Kil => IrOp::Kill,

                MaxwellOpcode::Exit => IrOp::Exit,

                _ => IrOp::Nop,
            };

            let is_terminal = matches!(ir_op, IrOp::Branch { .. } | IrOp::Exit | IrOp::Kill);
            current_block.push(ir_op);

            pc += 8;

            if is_terminal {
                program.blocks.push(current_block);
                current_block = ShaderBasicBlock::new(pc as u64);
            }
        }

        if !current_block.instructions.is_empty() {
            program.blocks.push(current_block);
        }

        program.analyze_resources();
        program
    }

    /// Emits WebGPU Shading Language (WGSL) string from decompiled ShaderProgram.
    pub fn emit_wgsl(program: &ShaderProgram) -> String {
        let mut out = String::new();
        out.push_str("// Decompiled from Maxwell SASS by Mockra Kagami (Starvald Labs)\n\n");

        match program.stage {
            ShaderStage::Vertex => {
                out.push_str("struct VertexInput {\n");
                for (idx, attr) in program.input_attributes.iter().enumerate() {
                    out.push_str(&format!("    @location({}) attr_{:02X}: vec4<f32>,\n", idx, attr));
                }
                out.push_str("};\n\n");

                out.push_str("struct VertexOutput {\n");
                out.push_str("    @builtin(position) position: vec4<f32>,\n");
                for (idx, attr) in program.output_attributes.iter().enumerate() {
                    out.push_str(&format!("    @location({}) out_attr_{:02X}: vec4<f32>,\n", idx, attr));
                }
                out.push_str("};\n\n");

                if !program.constant_buffers_used.is_empty() {
                    out.push_str("@group(0) @binding(0)\nvar<uniform> cbuf0: array<vec4<f32>, 256>;\n\n");
                }

                out.push_str("@vertex\nfn main(input: VertexInput) -> VertexOutput {\n");
                out.push_str("    var output: VertexOutput;\n");
                out.push_str("    var R: array<f32, 256>;\n\n");

                Self::emit_block_body(&mut out, program);

                out.push_str("    output.position = vec4<f32>(R[0], R[1], R[2], R[3]);\n");
                out.push_str("    return output;\n");
                out.push_str("}\n");
            }

            ShaderStage::Fragment => {
                if !program.samplers_used.is_empty() {
                    out.push_str("@group(0) @binding(0) var tex2d: texture_2d<f32>;\n");
                    out.push_str("@group(0) @binding(1) var samp: sampler;\n\n");
                }

                out.push_str("@fragment\nfn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {\n");
                out.push_str("    var R: array<f32, 256>;\n");
                out.push_str("    R[0] = uv.x;\n    R[1] = uv.y;\n\n");

                Self::emit_block_body(&mut out, program);

                out.push_str("    return vec4<f32>(R[0], R[1], R[2], R[3]);\n");
                out.push_str("}\n");
            }

            ShaderStage::Compute => {
                out.push_str("@compute @workgroup_size(16, 16, 1)\nfn main() {\n");
                out.push_str("    var R: array<f32, 256>;\n\n");
                Self::emit_block_body(&mut out, program);
                out.push_str("}\n");
            }

            ShaderStage::TessellationControl
            | ShaderStage::TessellationEvaluation
            | ShaderStage::Geometry => {
                out.push_str("// Tessellation / Geometry stage\nfn main() {\n");
                out.push_str("    var R: array<f32, 256>;\n\n");
                Self::emit_block_body(&mut out, program);
                out.push_str("}\n");
            }
        }

        out
    }

    fn emit_block_body(out: &mut String, program: &ShaderProgram) {
        for block in &program.blocks {
            out.push_str(&format!("    // Block at PC: 0x{:04X}\n", block.start_pc));
            for op in &block.instructions {
                match op {
                    IrOp::Move { dst, src } => {
                        out.push_str(&format!("    R[{}] = {};\n", dst, Self::operand_str(src)));
                    }
                    IrOp::FAdd { dst, src_a, src_b } => {
                        out.push_str(&format!("    R[{}] = {} + {};\n", dst, Self::operand_str(src_a), Self::operand_str(src_b)));
                    }
                    IrOp::FMul { dst, src_a, src_b } => {
                        out.push_str(&format!("    R[{}] = {} * {};\n", dst, Self::operand_str(src_a), Self::operand_str(src_b)));
                    }
                    IrOp::FFma { dst, src_a, src_b, src_c } => {
                        out.push_str(&format!("    R[{}] = ({} * {}) + {};\n", dst, Self::operand_str(src_a), Self::operand_str(src_b), Self::operand_str(src_c)));
                    }
                    IrOp::LoadConstant { dst, bank, offset } => {
                        out.push_str(&format!("    R[{}] = cbuf{}[{}].x;\n", dst, bank, offset / 16));
                    }
                    IrOp::TextureSample { dst_rgba, .. } => {
                        out.push_str(&format!("    let sampled = textureSample(tex2d, samp, uv);\n"));
                        out.push_str(&format!("    R[{}] = sampled.r;\n", dst_rgba[0]));
                        out.push_str(&format!("    R[{}] = sampled.g;\n", dst_rgba[1]));
                        out.push_str(&format!("    R[{}] = sampled.b;\n", dst_rgba[2]));
                        out.push_str(&format!("    R[{}] = sampled.a;\n", dst_rgba[3]));
                    }
                    IrOp::Kill => {
                        out.push_str("    discard;\n");
                    }
                    IrOp::Exit => {
                        out.push_str("    // Exit Shader\n");
                    }
                    _ => {}
                }
            }
        }
    }

    fn operand_str(operand: &SourceOperand) -> String {
        match operand {
            SourceOperand::Register(r) => format!("R[{}]", r),
            SourceOperand::ImmediateFloat(f) => format!("{:.4}", f),
            SourceOperand::ImmediateInt(i) => format!("{}", i),
            SourceOperand::ConstantBuffer { bank, offset } => format!("cbuf{}[{}].x", bank, offset / 16),
            SourceOperand::Attribute(attr) => format!("input.attr_{:02X}.x", attr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompile_vertex_shader_ffma_and_exit() {
        // Construct small Maxwell SASS program:
        // 1. FFMA R0, R1, R2, R3 (opcode 0x5980): R0 = R1 * R2 + R3
        // Rd=0, Ra=1, Rb=2, Rc=3
        let raw_ffma = (0x5980u64 << 48)
            | (3u64 << 40) // Rc = 3
            | (2u64 << 20) // Rb = 2
            | (7u64 << 16) // Predicate PT (True)
            | (1u64 << 8)  // Ra = 1
            | (0u64 << 0); // Rd = 0

        // 2. EXIT (opcode 0xE300)
        let raw_exit = (0xE300u64 << 48) | (7u64 << 16);

        let mut bytecode = Vec::new();
        bytecode.extend_from_slice(&raw_ffma.to_le_bytes());
        bytecode.extend_from_slice(&raw_exit.to_le_bytes());

        let program = ShaderDecompiler::decompile(&bytecode, ShaderStage::Vertex);
        assert_eq!(program.stage, ShaderStage::Vertex);
        assert_eq!(program.blocks.len(), 1);

        let block = &program.blocks[0];
        assert_eq!(block.instructions.len(), 2);
        assert!(matches!(block.instructions[0], IrOp::FFma { dst: 0, .. }));
        assert_eq!(block.instructions[1], IrOp::Exit);

        let wgsl = ShaderDecompiler::emit_wgsl(&program);
        assert!(wgsl.contains("@vertex"));
        assert!(wgsl.contains("R[0] = (R[1] * R[2]) + R[3];"));
    }

    #[test]
    fn test_decompile_fragment_shader_texture_sample() {
        // 1. TEX R0, R1, R2, sampler 0 (opcode 0xB000)
        let raw_tex = (0xB000u64 << 48)
            | (0u64 << 28) // Sampler 0
            | (2u64 << 20) // Rb (coord V) = 2
            | (7u64 << 16) // Predicate PT
            | (1u64 << 8)  // Ra (coord U) = 1
            | (0u64 << 0); // Rd = 0

        // 2. EXIT
        let raw_exit = (0xE300u64 << 48) | (7u64 << 16);

        let mut bytecode = Vec::new();
        bytecode.extend_from_slice(&raw_tex.to_le_bytes());
        bytecode.extend_from_slice(&raw_exit.to_le_bytes());

        let program = ShaderDecompiler::decompile(&bytecode, ShaderStage::Fragment);
        assert_eq!(program.samplers_used, vec![0]);

        let wgsl = ShaderDecompiler::emit_wgsl(&program);
        assert!(wgsl.contains("@fragment"));
        assert!(wgsl.contains("textureSample(tex2d, samp, uv)"));
    }
}
