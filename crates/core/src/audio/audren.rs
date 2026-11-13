//! Audio Renderer Service (`audren:u`, `audren:a`) implementation.
//!
//! Provides the primary DSP audio rendering pipeline used by modern Nintendo Switch games
//! (including Breath of the Wild, Super Mario Odyssey, and Tears of the Kingdom).
//! Handles multi-channel voice updates, command buffer updates, submixes, and system event signaling.

use super::dsp::{AudioDsp, VoiceState, WaveBuffer, MAX_VOICES, STANDARD_SAMPLE_RATE};
use crate::kernel::ipc::{IpcContext, IpcError, ServiceHandler};
use crate::kernel::object::KEvent;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRendererState {
    Started = 0,
    Stopped = 1,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioRendererConfig {
    pub sample_rate: u32,
    pub sample_count: u32,
    pub mix_buffer_count: u32,
    pub submix_count: u32,
    pub voice_count: u32,
    pub sink_count: u32,
    pub effect_count: u32,
    pub perf_frame_count: u32,
}

impl Default for AudioRendererConfig {
    fn default() -> Self {
        Self {
            sample_rate: STANDARD_SAMPLE_RATE,
            sample_count: 160,
            mix_buffer_count: 24,
            submix_count: 4,
            voice_count: 64,
            sink_count: 1,
            effect_count: 0,
            perf_frame_count: 0,
        }
    }
}

/// Active session representing an open `IAudioRenderer` instance.
pub struct IAudioRendererSession {
    dsp: Arc<Mutex<AudioDsp>>,
    config: AudioRendererConfig,
    state: Mutex<AudioRendererState>,
    pub system_event: KEvent,
}

impl IAudioRendererSession {
    pub fn new(dsp: Arc<Mutex<AudioDsp>>, config: AudioRendererConfig) -> Self {
        Self {
            dsp,
            config,
            state: Mutex::new(AudioRendererState::Stopped),
            system_event: KEvent::new(true), // auto-reset
        }
    }

    /// Ticks one audio rendering frame, mixing active voices in the DSP and signaling the system event.
    pub fn tick_frame(&self) -> (usize, Vec<u64>) {
        let mut dsp = self.dsp.lock().unwrap();
        dsp.process_frame(self.config.sample_count as usize);

        let mut all_released = Vec::new();
        for voice in &mut dsp.voices {
            if !voice.released_contexts.is_empty() {
                all_released.extend(std::mem::take(&mut voice.released_contexts));
            }
        }

        self.system_event.signal();
        (self.config.sample_count as usize, all_released)
    }
}

impl ServiceHandler for IAudioRendererSession {
    fn service_name(&self) -> &str {
        "IAudioRenderer"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetSampleRate() -> sample_rate: u32
            0 => {
                context.response.push_u32(self.config.sample_rate);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: GetSampleCount() -> sample_count: u32
            1 => {
                context.response.push_u32(self.config.sample_count);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: GetMixBufferCount() -> mix_buffers: u32
            2 => {
                context.response.push_u32(self.config.mix_buffer_count);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 3: GetState() -> state: u32
            3 => {
                let s = *self.state.lock().unwrap() as u32;
                context.response.push_u32(s);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 4: RequestUpdate(input_buffer) -> output_buffer
            // Heart of audren: parses voice update parameters, ticks DSP mixing,
            // and writes back released wave buffer tags and performance statistics.
            4 => {
                // If input buffer is provided, parse voice parameters
                let raw = &context.request.raw_data;
                if raw.len() >= 16 {
                    let voice_id = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
                    let voice_state = u32::from_le_bytes(raw[4..8].try_into().unwrap());
                    let volume_raw = u32::from_le_bytes(raw[8..12].try_into().unwrap());
                    let pitch_raw = u32::from_le_bytes(raw[12..16].try_into().unwrap());

                    let volume = f32::from_bits(volume_raw);
                    let pitch = f32::from_bits(pitch_raw);

                    if voice_id < MAX_VOICES {
                        let mut dsp = self.dsp.lock().unwrap();
                        let v = &mut dsp.voices[voice_id];
                        v.state = match voice_state {
                            1 => VoiceState::Playing,
                            2 => VoiceState::Paused,
                            _ => VoiceState::Stopped,
                        };
                        if volume > 0.0 {
                            v.volume = volume;
                        }
                        if pitch > 0.0 {
                            v.pitch = pitch;
                        }

                        // Check for inline wave buffer descriptor
                        if raw.len() >= 36 {
                            let buf_addr = u64::from_le_bytes(raw[16..24].try_into().unwrap());
                            let buf_size = u32::from_le_bytes(raw[24..28].try_into().unwrap());
                            let context_tag = u64::from_le_bytes(raw[28..36].try_into().unwrap());
                            let pcm_data = if raw.len() > 36 {
                                raw[36..].to_vec()
                            } else {
                                vec![0u8; buf_size as usize]
                            };

                            v.append_wave_buffer(WaveBuffer {
                                buffer_address: buf_addr,
                                data: pcm_data,
                                start_sample_offset: 0,
                                end_sample_offset: 0,
                                is_looping: false,
                                loop_start: 0,
                                loop_end: 0,
                                context: context_tag,
                            });
                        }
                    }
                }

                // If renderer is running, process a frame tick
                let is_running = *self.state.lock().unwrap() == AudioRendererState::Started;
                let (_samples, released_tags) = if is_running {
                    self.tick_frame()
                } else {
                    (0, Vec::new())
                };

                // Write output buffer: count of released tags + tags + performance metric cycles
                context.response.push_u32(released_tags.len() as u32);
                for tag in released_tags {
                    context.response.push_u64(tag);
                }
                context.response.push_u64(15000); // 15,000 elapsed DSP cycles estimate
                context.response.result_code = 0;
                Ok(())
            }

            // Command 5: Start()
            5 => {
                *self.state.lock().unwrap() = AudioRendererState::Started;
                context.response.result_code = 0;
                Ok(())
            }

            // Command 6: Stop()
            6 => {
                *self.state.lock().unwrap() = AudioRendererState::Stopped;
                context.response.result_code = 0;
                Ok(())
            }

            // Command 7: QuerySystemEvent() -> out_event_handle
            7 => {
                context.spawned_events.push(self.system_event.clone());
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

/// Active session representing an open `IAudioDevice` instance.
pub struct IAudioDeviceSession {
    dsp: Arc<Mutex<AudioDsp>>,
    current_device: Mutex<String>,
}

impl IAudioDeviceSession {
    pub fn new(dsp: Arc<Mutex<AudioDsp>>) -> Self {
        Self {
            dsp,
            current_device: Mutex::new("AudioTvOutput".into()),
        }
    }
}

impl ServiceHandler for IAudioDeviceSession {
    fn service_name(&self) -> &str {
        "IAudioDevice"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: ListAudioDeviceName()
            0 => {
                let names = b"AudioTvOutput\0AudioBuiltInSpeaker\0AudioHeadphoneOutput\0";
                context.response.push_u32(names.len() as u32);
                context.response.push_bytes(names);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: SetAudioDeviceOutputVolume(volume: f32)
            1 => {
                if let Some(vol_bits) = context.request.read_u32(0) {
                    let vol = f32::from_bits(vol_bits);
                    self.dsp.lock().unwrap().master_volume = vol.clamp(0.0, 2.0);
                }
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: GetAudioDeviceOutputVolume() -> volume: f32
            2 => {
                let vol = self.dsp.lock().unwrap().master_volume;
                context.response.push_u32(vol.to_bits());
                context.response.result_code = 0;
                Ok(())
            }

            // Command 3: GetActiveAudioDeviceName() -> name: str
            3 => {
                let name = self.current_device.lock().unwrap().clone();
                context.response.push_bytes(name.as_bytes());
                context.response.push_bytes(&[0]);
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

/// Service Manager factory handler for `audren:u`, `audren:a`, `audren:d`.
pub struct AudioRendererService {
    dsp: Arc<Mutex<AudioDsp>>,
}

impl Default for AudioRendererService {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioRendererService {
    pub fn new() -> Self {
        Self {
            dsp: Arc::new(Mutex::new(AudioDsp::new())),
        }
    }

    pub fn with_dsp(dsp: Arc<Mutex<AudioDsp>>) -> Self {
        Self { dsp }
    }
}

impl ServiceHandler for AudioRendererService {
    fn service_name(&self) -> &str {
        "audren:u"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: OpenAudioRenderer(config) -> out_session_handle, out_event_handle
            0 => {
                let req_sample_rate = context.request.read_u32(0).unwrap_or(STANDARD_SAMPLE_RATE);
                let sample_rate = if req_sample_rate == 0 { STANDARD_SAMPLE_RATE } else { req_sample_rate };
                let sample_count = context.request.read_u32(4).unwrap_or(160);
                let mix_buffer_count = context.request.read_u32(8).unwrap_or(24);
                let voice_count = context.request.read_u32(12).unwrap_or(64);

                let config = AudioRendererConfig {
                    sample_rate,
                    sample_count: if sample_count == 0 { 160 } else { sample_count },
                    mix_buffer_count: if mix_buffer_count == 0 { 24 } else { mix_buffer_count },
                    voice_count: if voice_count == 0 { 64 } else { voice_count },
                    ..AudioRendererConfig::default()
                };

                let session = Arc::new(IAudioRendererSession::new(self.dsp.clone(), config));
                context.spawned_sessions.push(("IAudioRenderer".into(), session.clone()));
                context.spawned_events.push(session.system_event.clone());

                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: GetAudioDeviceService() -> out_session_handle
            1 => {
                let dev_session = Arc::new(IAudioDeviceSession::new(self.dsp.clone()));
                context.spawned_sessions.push(("IAudioDevice".into(), dev_session));
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: OpenAudioRendererForManualExecution(config)
            2 => {
                let config = AudioRendererConfig::default();
                let session = Arc::new(IAudioRendererSession::new(self.dsp.clone(), config));
                context.spawned_sessions.push(("IAudioRenderer".into(), session.clone()));
                context.spawned_events.push(session.system_event.clone());

                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ipc::CmifRequest;

    #[test]
    fn test_audren_open_and_request_update_lifecycle() {
        let service = AudioRendererService::new();

        // 1. OpenAudioRenderer
        let mut cfg_data = Vec::new();
        cfg_data.extend_from_slice(&48000u32.to_le_bytes()); // sample_rate
        cfg_data.extend_from_slice(&160u32.to_le_bytes());   // sample_count
        cfg_data.extend_from_slice(&24u32.to_le_bytes());    // mix_buffers
        cfg_data.extend_from_slice(&32u32.to_le_bytes());    // voices

        let req_open = CmifRequest::new(0, cfg_data);
        let mut ctx_open = IpcContext::new(req_open);
        service.dispatch(&mut ctx_open).unwrap();

        assert_eq!(ctx_open.response.result_code, 0);
        assert_eq!(ctx_open.spawned_sessions.len(), 1);
        assert_eq!(ctx_open.spawned_events.len(), 1);

        let renderer_session = ctx_open.spawned_sessions[0].1.clone();

        // 2. Start renderer
        let req_start = CmifRequest::new(5, vec![]);
        let mut ctx_start = IpcContext::new(req_start);
        renderer_session.dispatch(&mut ctx_start).unwrap();
        assert_eq!(ctx_start.response.result_code, 0);

        // 3. RequestUpdate with voice playback
        let mut update_data = Vec::new();
        update_data.extend_from_slice(&0u32.to_le_bytes()); // voice_id 0
        update_data.extend_from_slice(&1u32.to_le_bytes()); // Playing state
        update_data.extend_from_slice(&1.0f32.to_bits().to_le_bytes()); // volume
        update_data.extend_from_slice(&1.0f32.to_bits().to_le_bytes()); // pitch
        // Inline wave buffer descriptor (160 samples mono = 320 bytes)
        update_data.extend_from_slice(&0x8000_0000u64.to_le_bytes()); // buf_addr
        update_data.extend_from_slice(&320u32.to_le_bytes()); // size
        update_data.extend_from_slice(&0xFEED_FACEu64.to_le_bytes()); // context tag
        update_data.extend_from_slice(&vec![0x7F; 320]); // sample bytes

        let req_update = CmifRequest::new(4, update_data);
        let mut ctx_update = IpcContext::new(req_update);
        renderer_session.dispatch(&mut ctx_update).unwrap();

        assert_eq!(ctx_update.response.result_code, 0);
        // Released wave buffer tag should be present
        let released_count = u32::from_le_bytes(ctx_update.response.raw_data[..4].try_into().unwrap());
        assert_eq!(released_count, 1);
        let tag = u64::from_le_bytes(ctx_update.response.raw_data[4..12].try_into().unwrap());
        assert_eq!(tag, 0xFEED_FACE);
    }

    #[test]
    fn test_audio_device_volume_control() {
        let dsp = Arc::new(Mutex::new(AudioDsp::new()));
        let dev = IAudioDeviceSession::new(dsp.clone());

        // Set volume to 0.75
        let vol_bits = 0.75f32.to_bits();
        let req_set = CmifRequest::new(1, vol_bits.to_le_bytes().to_vec());
        let mut ctx_set = IpcContext::new(req_set);
        dev.dispatch(&mut ctx_set).unwrap();

        assert_eq!(dsp.lock().unwrap().master_volume, 0.75);

        // Get volume
        let req_get = CmifRequest::new(2, vec![]);
        let mut ctx_get = IpcContext::new(req_get);
        dev.dispatch(&mut ctx_get).unwrap();

        let returned_vol_bits = u32::from_le_bytes(ctx_get.response.raw_data[..4].try_into().unwrap());
        assert_eq!(f32::from_bits(returned_vol_bits), 0.75);
    }
}
