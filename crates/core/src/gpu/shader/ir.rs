//! Intermediate Representation (NVIR) for Maxwell Shaders.

use super::instruction::Predicate;
use crate::gpu::shader_cache::ShaderStage;
use std::collections::HashSet;

/// Source operand for intermediate shader instructions.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceOperand {
    Register(u8),
    ImmediateFloat(f32),
    ImmediateInt(u32),
    ConstantBuffer { bank: u16, offset: u32 },
    Attribute(u32),
}

/// Abstract IR Operation.
#[derive(Debug, Clone, PartialEq)]
pub enum IrOp {
    Move {
        dst: u8,
        src: SourceOperand,
    },
    FAdd {
        dst: u8,
        src_a: SourceOperand,
        src_b: SourceOperand,
    },
    FMul {
        dst: u8,
        src_a: SourceOperand,
        src_b: SourceOperand,
    },
    FFma {
        dst: u8,
        src_a: SourceOperand,
        src_b: SourceOperand,
        src_c: SourceOperand,
    },
    FMinMax {
        dst: u8,
        src_a: SourceOperand,
        src_b: SourceOperand,
        is_max: bool,
    },
    LoadConstant {
        dst: u8,
        bank: u16,
        offset: u32,
    },
    LoadAttribute {
        dst: u8,
        attr_offset: u32,
    },
    StoreAttribute {
        attr_offset: u32,
        src: SourceOperand,
    },
    TextureSample {
        dst_rgba: [u8; 4],
        sampler_index: u32,
        coord_u: SourceOperand,
        coord_v: SourceOperand,
    },
    Branch {
        target_pc: u64,
    },
    BranchCond {
        pred: Predicate,
        target_pc: u64,
    },
    Kill,
    Exit,
    Nop,
}

/// Linear basic block of shader instructions.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderBasicBlock {
    pub start_pc: u64,
    pub instructions: Vec<IrOp>,
}

impl ShaderBasicBlock {
    pub fn new(start_pc: u64) -> Self {
        Self {
            start_pc,
            instructions: Vec::new(),
        }
    }

    pub fn push(&mut self, op: IrOp) {
        self.instructions.push(op);
    }
}

/// High-level decompiled shader program representation.
#[derive(Debug, Clone)]
pub struct ShaderProgram {
    pub stage: ShaderStage,
    pub blocks: Vec<ShaderBasicBlock>,
    pub input_attributes: Vec<u32>,
    pub output_attributes: Vec<u32>,
    pub constant_buffers_used: Vec<(u16, u32)>,
    pub samplers_used: Vec<u32>,
}

impl ShaderProgram {
    pub fn new(stage: ShaderStage) -> Self {
        Self {
            stage,
            blocks: Vec::new(),
            input_attributes: Vec::new(),
            output_attributes: Vec::new(),
            constant_buffers_used: Vec::new(),
            samplers_used: Vec::new(),
        }
    }

    /// Records resource usage across all basic blocks.
    pub fn analyze_resources(&mut self) {
        let mut in_attrs = HashSet::new();
        let mut out_attrs = HashSet::new();
        let mut cbufs = HashSet::new();
        let mut samplers = HashSet::new();

        for block in &self.blocks {
            for op in &block.instructions {
                match op {
                    IrOp::LoadAttribute { attr_offset, .. } => {
                        in_attrs.insert(*attr_offset);
                    }
                    IrOp::StoreAttribute { attr_offset, .. } => {
                        out_attrs.insert(*attr_offset);
                    }
                    IrOp::LoadConstant { bank, offset, .. } => {
                        cbufs.insert((*bank, *offset));
                    }
                    IrOp::TextureSample { sampler_index, .. } => {
                        samplers.insert(*sampler_index);
                    }
                    IrOp::FAdd { src_a, src_b, .. }
                    | IrOp::FMul { src_a, src_b, .. } => {
                        Self::inspect_operand(src_a, &mut in_attrs, &mut cbufs);
                        Self::inspect_operand(src_b, &mut in_attrs, &mut cbufs);
                    }
                    IrOp::FFma { src_a, src_b, src_c, .. } => {
                        Self::inspect_operand(src_a, &mut in_attrs, &mut cbufs);
                        Self::inspect_operand(src_b, &mut in_attrs, &mut cbufs);
                        Self::inspect_operand(src_c, &mut in_attrs, &mut cbufs);
                    }
                    _ => {}
                }
            }
        }

        self.input_attributes = in_attrs.into_iter().collect();
        self.input_attributes.sort_unstable();
        self.output_attributes = out_attrs.into_iter().collect();
        self.output_attributes.sort_unstable();
        self.constant_buffers_used = cbufs.into_iter().collect();
        self.constant_buffers_used.sort_unstable();
        self.samplers_used = samplers.into_iter().collect();
        self.samplers_used.sort_unstable();
    }

    fn inspect_operand(
        operand: &SourceOperand,
        in_attrs: &mut HashSet<u32>,
        cbufs: &mut HashSet<(u16, u32)>,
    ) {
        match operand {
            SourceOperand::Attribute(attr) => {
                in_attrs.insert(*attr);
            }
            SourceOperand::ConstantBuffer { bank, offset } => {
                cbufs.insert((*bank, *offset));
            }
            _ => {}
        }
    }
}
