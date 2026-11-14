//! End-to-end integration tests for Tegra Block-Linear Texture Swizzling
//! and Maxwell SASS Shader Recompiler & Decompiler Pipeline.

use emu_core::gpu::shader::ShaderDecompiler;
use emu_core::gpu::texture::{calculate_block_linear_size, swizzle_pitch_to_block_linear};
use emu_core::gpu::{ColorFormat, DisplaySurface, ShaderStage};
use emu_core::memory::fastmem::{permissions, Fastmem};
use emu_core::memory::MemoryBus;

#[test]
fn test_tegra_block_linear_vram_and_presentation_pipeline() {
    let width = 128;
    let height = 64;
    let bpp = 4; // RGBA8
    let block_height_gobs = 4; // 4 GOBs = 32 lines per block

    // 1. Create source procedural test texture (diagonal color ramp)
    let mut linear_pixels = vec![0u8; width * height * bpp];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * bpp;
            linear_pixels[idx + 0] = (x * 2) as u8;       // R
            linear_pixels[idx + 1] = (y * 4) as u8;       // G
            linear_pixels[idx + 2] = ((x + y) * 3) as u8; // B
            linear_pixels[idx + 3] = 255;                 // A
        }
    }

    // 2. Swizzle into Tegra block-linear hardware layout
    let swizzled = swizzle_pitch_to_block_linear(
        &linear_pixels,
        width,
        height,
        bpp,
        block_height_gobs,
    );
    let expected_tiled_size = calculate_block_linear_size(width, height, bpp, block_height_gobs);
    assert_eq!(swizzled.len(), expected_tiled_size);

    // 3. Map into GPU VRAM (Fastmem paged memory at 0x9000_0000)
    let vram_base = 0x9000_0000;
    let mut fastmem = Fastmem::new();
    fastmem.map_range(vram_base, 0x100000, permissions::RWX);

    for (i, &byte) in swizzled.iter().enumerate() {
        fastmem.write_u8(vram_base + i as u64, byte).unwrap();
    }

    // 4. Configure Horizon VI DisplaySurface with block-linear layout
    let display = DisplaySurface::new(
        width as u32,
        height as u32,
        ColorFormat::Rgba8Unorm,
        vram_base,
    )
    .with_block_linear(block_height_gobs);

    // 5. Extract presentation frame and verify byte-for-byte fidelity with the source image
    let presented_frame = display.extract_presentation_frame(&fastmem).unwrap();
    assert_eq!(presented_frame.len(), linear_pixels.len());
    assert_eq!(presented_frame, linear_pixels);
}

#[test]
fn test_maxwell_vertex_and_fragment_shader_decompilation_pipeline() {
    // --- Vertex Shader SASS Bytecode ---
    // 1. ALD R0, attr[0x80] (Opcode 0xEFD8, Attribute 0x80 = 32 * 4)
    let raw_ald_x = (0xEFD8u64 << 48)
        | (32u64 << 20) // attr offset / 4 = 32 (0x80)
        | (7u64 << 16)  // Predicate PT
        | (0u64 << 0);  // Rd = 0

    // 2. LDC R2, c[0x0][0x40] (Opcode 0xEF90, bank 0, offset 0x40 = 16 * 4)
    let raw_ldc = (0xEF90u64 << 48)
        | (16u64 << 28) // offset / 4 = 16
        | (0u64 << 24)  // bank = 0
        | (7u64 << 16)  // Predicate PT
        | (2u64 << 0);  // Rd = 2

    // 3. FMUL R0, R0, R2 (Opcode 0x5C68)
    let raw_fmul = (0x5C68u64 << 48)
        | (2u64 << 20) // Rb = 2
        | (7u64 << 16) // Predicate PT
        | (0u64 << 8)  // Ra = 0
        | (0u64 << 0); // Rd = 0

    // 4. AST attr[0x80], R0 (Opcode 0xEFF0)
    let raw_ast_x = (0xEFF0u64 << 48)
        | (32u64 << 20) // attr offset = 0x80
        | (7u64 << 16)  // Predicate PT
        | (0u64 << 8);  // Ra = 0

    // 5. EXIT (Opcode 0xE300)
    let raw_exit = (0xE300u64 << 48) | (7u64 << 16);

    let mut vs_bytecode = Vec::new();
    vs_bytecode.extend_from_slice(&raw_ald_x.to_le_bytes());
    vs_bytecode.extend_from_slice(&raw_ldc.to_le_bytes());
    vs_bytecode.extend_from_slice(&raw_fmul.to_le_bytes());
    vs_bytecode.extend_from_slice(&raw_ast_x.to_le_bytes());
    vs_bytecode.extend_from_slice(&raw_exit.to_le_bytes());

    let vs_program = ShaderDecompiler::decompile(&vs_bytecode, ShaderStage::Vertex);
    assert_eq!(vs_program.stage, ShaderStage::Vertex);
    assert_eq!(vs_program.input_attributes, vec![0x80]);
    assert_eq!(vs_program.output_attributes, vec![0x80]);
    assert_eq!(vs_program.constant_buffers_used, vec![(0, 0x40)]);

    let vs_wgsl = ShaderDecompiler::emit_wgsl(&vs_program);
    assert!(vs_wgsl.contains("@vertex"));
    assert!(vs_wgsl.contains("attr_80: vec4<f32>"));
    assert!(vs_wgsl.contains("cbuf0"));
    assert!(vs_wgsl.contains("R[0] = R[0] * R[2];"));

    // --- Fragment Shader SASS Bytecode ---
    // 1. TEX R4, R0, R1, sampler 2 (Opcode 0xB000)
    let raw_tex = (0xB000u64 << 48)
        | (2u64 << 28) // Sampler 2
        | (1u64 << 20) // Rb (coord V) = 1
        | (7u64 << 16) // Predicate PT
        | (0u64 << 8)  // Ra (coord U) = 0
        | (4u64 << 0); // Rd = 4

    // 2. EXIT
    let mut fs_bytecode = Vec::new();
    fs_bytecode.extend_from_slice(&raw_tex.to_le_bytes());
    fs_bytecode.extend_from_slice(&raw_exit.to_le_bytes());

    let fs_program = ShaderDecompiler::decompile(&fs_bytecode, ShaderStage::Fragment);
    assert_eq!(fs_program.stage, ShaderStage::Fragment);
    assert_eq!(fs_program.samplers_used, vec![2]);

    let fs_wgsl = ShaderDecompiler::emit_wgsl(&fs_program);
    assert!(fs_wgsl.contains("@fragment"));
    assert!(fs_wgsl.contains("textureSample(tex2d, samp, uv)"));
}
