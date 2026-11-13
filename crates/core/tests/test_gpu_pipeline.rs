use emu_core::cpu::Cpu;
use emu_core::gpu::{
    methods, BufferCache, ColorFormat, DisplaySurface, GpfifoProcessor, Maxwell3D, ShaderCache,
    ShaderStage,
};
use emu_core::kernel::KernelContext;
use emu_core::memory::{FlatMemory, MemoryBus};

#[test]
fn test_nvdrv_service_ipc_and_nvmap_allocation() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new(0, 0x20000);
    let mut kernel = KernelContext::new(false);

    let sm_handle = kernel.sm_session_handle;
    let tls_addr = 0x1000;
    kernel.tls_base = tls_addr;

    // 1. Request "nvdrv" service session from sm:
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 1).unwrap(); // Cmd 1: GetService
    for (i, &b) in b"nvdrv\0\0\0".iter().take(8).enumerate() {
        bus.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let nvdrv_handle = bus.read_u32(tls_addr + 16).unwrap();
    assert_ne!(nvdrv_handle, 0);

    // 2. Open "/dev/nvmap" on nvdrv session (Cmd 0: Open)
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 8, 0).unwrap(); // Cmd 0: Open
    for (i, &b) in b"/dev/nvmap\0".iter().take(10).enumerate() {
        bus.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }

    cpu.regs.set_x(0, nvdrv_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let nvmap_fd = bus.read_u32(tls_addr + 16).unwrap();
    assert_eq!(nvmap_fd, 1);
}

#[test]
fn test_gpu_pushbuffer_clear_and_display_presentation() {
    let mut bus = FlatMemory::new(0, 0x10_0000); // 1 MB RAM
    let fb_address = 0x10000;
    let width = 64;
    let height = 64;

    let mut maxwell = Maxwell3D::new();
    let mut fifo = GpfifoProcessor::new();
    let mut buffer_cache = BufferCache::new();

    // Register render target in buffer cache
    let fb_size = (width * height * 4) as usize;
    buffer_cache.register_buffer(fb_address, fb_size, true);
    assert!(buffer_cache.is_dirty(fb_address, fb_size));

    // Construct Maxwell GPFIFO Pushbuffer:
    // Packet 1: Set Render Target parameters (address low/high, width, height)
    // IncMethod (0 << 29), Count = 4, Starting at RT_ADDRESS_HIGH (0x0080)
    let p1_header = (4 << 16) | methods::RT_ADDRESS_HIGH;
    let p1_words = vec![
        p1_header,
        0,                 // RT_ADDRESS_HIGH = 0
        fb_address as u32, // RT_ADDRESS_LOW = fb_address
        width,             // RT_WIDTH
        height,            // RT_HEIGHT
    ];
    fifo.process_pushbuffer(&mut maxwell, &p1_words, &mut bus).unwrap();

    // Packet 2: Set Clear Color to Nintendo Red: RGBA (0.9, 0.0, 0.07, 1.0)
    // IncMethod, Count = 4, Starting at CLEAR_COLOR_R (0x00D8)
    let p2_header = (4 << 16) | methods::CLEAR_COLOR_R;
    let p2_words = vec![
        p2_header,
        0.9f32.to_bits(),
        0.0f32.to_bits(),
        0.07f32.to_bits(),
        1.0f32.to_bits(),
    ];
    fifo.process_pushbuffer(&mut maxwell, &p2_words, &mut bus).unwrap();

    // Packet 3: Trigger CLEAR_SURFACE (0x00DD)
    // NonIncMethod (1 << 29), Count = 1, Method = 0x00DD
    let p3_header = (1 << 29) | (1 << 16) | methods::CLEAR_SURFACE;
    let p3_words = vec![p3_header, 1];
    fifo.process_pushbuffer(&mut maxwell, &p3_words, &mut bus).unwrap();

    // Verify framebuffer memory was filled with Nintendo Red:
    let r = bus.read_u8(fb_address).unwrap();
    let g = bus.read_u8(fb_address + 1).unwrap();
    let b = bus.read_u8(fb_address + 2).unwrap();
    let a = bus.read_u8(fb_address + 3).unwrap();

    assert!(r > 220); // ~229
    assert_eq!(g, 0);
    assert!(b > 10 && b < 25); // ~17
    assert_eq!(a, 255);

    // Extract frame via DisplaySurface
    let surface = DisplaySurface::new(width, height, ColorFormat::Rgba8Unorm, fb_address);
    let pixels = surface.extract_pixels(&bus).unwrap();
    assert_eq!(pixels.len(), fb_size);
    assert_eq!(pixels[0..4], [r, g, b, a]);

    // Clear dirty status after display extraction
    buffer_cache.clear_dirty(fb_address, fb_size);
    assert!(!buffer_cache.is_dirty(fb_address, fb_size));
}

#[test]
fn test_shader_cache_deduplication() {
    let mut cache = ShaderCache::new();

    // Simulated Maxwell Fragment Shader Bytecode
    let fragment_shader = vec![
        0x01, 0x00, 0x00, 0x5C, 0x00, 0x00, 0x00, 0x00,
        0x02, 0x00, 0x00, 0xE3, 0x00, 0x00, 0x00, 0x00,
    ];

    let info1 = cache.register(ShaderStage::Fragment, 0x7100_5000, &fragment_shader);
    let info2 = cache.register(ShaderStage::Fragment, 0x7100_6000, &fragment_shader);

    // Both registrations have identical bytecode hash
    assert_eq!(info1.hash, info2.hash);
    assert_eq!(cache.len(), 1); // Deduplicated
}
