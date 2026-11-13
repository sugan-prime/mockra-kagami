//! End-to-end integration tests for Horizon OS Audio Subsystem (`audout:u`, `audren:u`, DSP).

use emu_core::cpu::Cpu;
use emu_core::kernel::KernelContext;
use emu_core::memory::{FlatMemory, MemoryBus};

/// Helper to serialize a CMIF request into guest TLS buffer.
fn write_cmif_request(bus: &mut FlatMemory, tls_addr: u64, command_id: u32, raw_payload: &[u8]) {
    bus.write_u8(tls_addr + 0, b'S').unwrap();
    bus.write_u8(tls_addr + 1, b'F').unwrap();
    bus.write_u8(tls_addr + 2, b'C').unwrap();
    bus.write_u8(tls_addr + 3, b'I').unwrap();
    bus.write_u32(tls_addr + 4, 0).unwrap(); // version
    bus.write_u32(tls_addr + 8, command_id).unwrap();
    bus.write_u32(tls_addr + 12, 0).unwrap(); // padding

    for (i, &b) in raw_payload.iter().enumerate() {
        bus.write_u8(tls_addr + 16 + i as u64, b).unwrap();
    }
}

/// Helper to read response payload from TLS.
fn read_cmif_response(bus: &FlatMemory, tls_addr: u64) -> (u32, Vec<u8>) {
    let mut magic = [0u8; 4];
    for (i, slot) in magic.iter_mut().enumerate() {
        *slot = bus.read_u8(tls_addr + i as u64).unwrap();
    }
    assert_eq!(&magic, b"SFCO");

    let result_code = bus.read_u32(tls_addr + 8).unwrap();
    let mut payload = vec![0u8; 64];
    for (i, slot) in payload.iter_mut().enumerate() {
        *slot = bus.read_u8(tls_addr + 16 + i as u64).unwrap();
    }
    (result_code, payload)
}

#[test]
fn test_sm_audout_service_and_pcm_stream() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new(0, 0x40000);
    let mut kernel = KernelContext::new(false);

    let tls_addr = 0x1000;
    kernel.tls_base = tls_addr;
    let sm_handle = kernel.sm_session_handle;

    // 1. sm:GetService("audout:u")
    let mut name_bytes = [0u8; 8];
    name_bytes[..8].copy_from_slice(b"audout:u");
    write_cmif_request(&mut bus, tls_addr, 1, &name_bytes);
    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let audout_handle = bus.read_u32(tls_addr + 16).unwrap();
    assert_ne!(audout_handle, 0);

    // 2. audout:u -> OpenAudioOut(sample_rate=48000)
    let mut open_payload = Vec::new();
    open_payload.extend_from_slice(&48000u32.to_le_bytes());
    open_payload.extend_from_slice(&2u16.to_le_bytes()); // stereo
    open_payload.extend_from_slice(b"AudioTvOutput\0");

    write_cmif_request(&mut bus, tls_addr, 1, &open_payload);
    cpu.regs.set_x(0, audout_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let sample_rate = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    let channels = u32::from_le_bytes(payload[4..8].try_into().unwrap());
    let session_h = u32::from_le_bytes(payload[16..20].try_into().unwrap());
    let event_h = u32::from_le_bytes(payload[20..24].try_into().unwrap());

    assert_eq!(sample_rate, 48000);
    assert_eq!(channels, 2);
    assert!(session_h > 0);
    assert!(event_h > 0);

    // 3. IAudioOut -> AppendAudioOutBuffer
    let mut append_payload = Vec::new();
    append_payload.extend_from_slice(&0x2000_0000u64.to_le_bytes()); // buffer_ptr
    append_payload.extend_from_slice(&640u64.to_le_bytes());         // size
    append_payload.extend_from_slice(&0x1122_3344u64.to_le_bytes()); // user tag
    append_payload.extend_from_slice(&vec![0xAA; 640]);             // PCM sample data

    write_cmif_request(&mut bus, tls_addr, 3, &append_payload);
    cpu.regs.set_x(0, session_h as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    // 4. IAudioOut -> StartAudioOut
    write_cmif_request(&mut bus, tls_addr, 1, &[]);
    cpu.regs.set_x(0, session_h as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    // 5. IAudioOut -> ContainsAudioOutBuffer
    write_cmif_request(&mut bus, tls_addr, 6, &0x1122_3344u64.to_le_bytes());
    cpu.regs.set_x(0, session_h as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);
    let contains = bus.read_u32(tls_addr + 16).unwrap();
    assert_eq!(contains, 1);
}

#[test]
fn test_sm_audren_service_and_dsp_mixer() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new(0, 0x40000);
    let mut kernel = KernelContext::new(false);

    let tls_addr = 0x1000;
    kernel.tls_base = tls_addr;
    let sm_handle = kernel.sm_session_handle;

    // 1. sm:GetService("audren:u")
    let mut name_bytes = [0u8; 8];
    name_bytes[..8].copy_from_slice(b"audren:u");
    write_cmif_request(&mut bus, tls_addr, 1, &name_bytes);
    cpu.regs.set_x(0, sm_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let audren_handle = bus.read_u32(tls_addr + 16).unwrap();
    assert_ne!(audren_handle, 0);

    // 2. OpenAudioRenderer(sample_rate=48000, sample_count=160, mix_buffers=24, voices=64)
    let mut cfg = Vec::new();
    cfg.extend_from_slice(&48000u32.to_le_bytes());
    cfg.extend_from_slice(&160u32.to_le_bytes());
    cfg.extend_from_slice(&24u32.to_le_bytes());
    cfg.extend_from_slice(&64u32.to_le_bytes());

    write_cmif_request(&mut bus, tls_addr, 0, &cfg);
    cpu.regs.set_x(0, audren_handle as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);
    let session_h = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    let event_h = u32::from_le_bytes(payload[4..8].try_into().unwrap());
    assert!(session_h > 0);
    assert!(event_h > 0);

    // 3. Start Audio Renderer (Cmd 5)
    write_cmif_request(&mut bus, tls_addr, 5, &[]);
    cpu.regs.set_x(0, session_h as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    // 4. RequestUpdate with 440 Hz Sine wave audio payload
    let samples_count = 160;
    let mut pcm_voice0 = Vec::new();
    for i in 0..samples_count {
        let t = (i as f32) / 48000.0;
        let s = ((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 12000.0) as i16;
        pcm_voice0.extend_from_slice(&s.to_le_bytes());
    }

    let mut update_data = Vec::new();
    update_data.extend_from_slice(&0u32.to_le_bytes()); // voice 0
    update_data.extend_from_slice(&1u32.to_le_bytes()); // Playing
    update_data.extend_from_slice(&1.0f32.to_bits().to_le_bytes()); // volume
    update_data.extend_from_slice(&1.0f32.to_bits().to_le_bytes()); // pitch
    update_data.extend_from_slice(&0x3000_0000u64.to_le_bytes());  // addr
    update_data.extend_from_slice(&(pcm_voice0.len() as u32).to_le_bytes());
    update_data.extend_from_slice(&0xAAAA_BBBB_CCCC_DDDDu64.to_le_bytes()); // context
    update_data.extend_from_slice(&pcm_voice0);

    write_cmif_request(&mut bus, tls_addr, 4, &update_data);
    cpu.regs.set_x(0, session_h as u64);
    kernel.handle_svc(0x21, &mut cpu, &mut bus).unwrap();
    assert_eq!(cpu.regs.get_x(0), 0);

    let (res, payload) = read_cmif_response(&bus, tls_addr);
    assert_eq!(res, 0);

    let released_count = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    assert_eq!(released_count, 1);
    let tag = u64::from_le_bytes(payload[4..12].try_into().unwrap());
    assert_eq!(tag, 0xAAAA_BBBB_CCCC_DDDD);

    // 5. Verify system event was signaled
    let event_obj = kernel.handles.get(event_h).unwrap();
    assert!(event_obj.is_signaled(), "System event should be signaled on frame tick");
}
