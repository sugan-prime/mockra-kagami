//! Audio Out Service (`audout:u`, `audout:a`) implementation.
//!
//! Provides the primary PCM output audio streaming interface used by Switch games
//! for direct audio playback, headphone/speaker routing, and buffer completion events.

use super::dsp::{AudioDsp, WaveBuffer, STANDARD_SAMPLE_RATE};
use crate::kernel::ipc::{IpcContext, IpcError, ServiceHandler};
use crate::kernel::object::KEvent;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOutState {
    Started = 0,
    Stopped = 1,
}

#[derive(Debug, Clone)]
pub struct AudioOutBuffer {
    pub buffer_ptr: u64,
    pub buffer_size: u64,
    pub data_tag: u64,
    pub sample_data: Vec<u8>,
}

/// Active session representing an open audio output device.
pub struct IAudioOutSession {
    dsp: Arc<Mutex<AudioDsp>>,
    state: Mutex<AudioOutState>,
    pub buffer_event: KEvent,
    queued_buffers: Mutex<VecDeque<AudioOutBuffer>>,
    released_buffers: Mutex<Vec<u64>>,
    voice_id: usize,
}

impl IAudioOutSession {
    pub fn new(dsp: Arc<Mutex<AudioDsp>>, voice_id: usize) -> Self {
        Self {
            dsp,
            state: Mutex::new(AudioOutState::Stopped),
            buffer_event: KEvent::new(true), // auto-reset
            queued_buffers: Mutex::new(VecDeque::new()),
            released_buffers: Mutex::new(Vec::new()),
            voice_id,
        }
    }

    /// Appends a buffer with sample data and queues it for playback.
    pub fn queue_buffer(&self, buf: AudioOutBuffer) {
        let tag = buf.data_tag;
        let pcm = buf.sample_data.clone();

        {
            let mut dsp = self.dsp.lock().unwrap();
            let voice = &mut dsp.voices[self.voice_id];
            voice.append_wave_buffer(WaveBuffer {
                buffer_address: buf.buffer_ptr,
                data: pcm,
                start_sample_offset: 0,
                end_sample_offset: 0,
                is_looping: false,
                loop_start: 0,
                loop_end: 0,
                context: tag,
            });
        }

        self.queued_buffers.lock().unwrap().push_back(buf);
    }

    /// Checks for any wave buffers completed by the DSP, moves them to released queue,
    /// and signals `buffer_event`.
    pub fn poll_released_buffers(&self) -> usize {
        let mut dsp = self.dsp.lock().unwrap();
        let voice = &mut dsp.voices[self.voice_id];

        if voice.released_contexts.is_empty() {
            return 0;
        }

        let completed = std::mem::take(&mut voice.released_contexts);
        let count = completed.len();

        let mut queued = self.queued_buffers.lock().unwrap();
        let mut released = self.released_buffers.lock().unwrap();

        for tag in completed {
            queued.retain(|b| b.data_tag != tag);
            released.push(tag);
        }

        self.buffer_event.signal();
        count
    }
}

impl ServiceHandler for IAudioOutSession {
    fn service_name(&self) -> &str {
        "IAudioOut"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetAudioOutState() -> state: u32
            0 => {
                let s = *self.state.lock().unwrap() as u32;
                context.response.push_u32(s);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: StartAudioOut()
            1 => {
                *self.state.lock().unwrap() = AudioOutState::Started;
                let mut dsp = self.dsp.lock().unwrap();
                dsp.voices[self.voice_id].state = super::dsp::VoiceState::Playing;
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: StopAudioOut()
            2 => {
                *self.state.lock().unwrap() = AudioOutState::Stopped;
                let mut dsp = self.dsp.lock().unwrap();
                dsp.voices[self.voice_id].state = super::dsp::VoiceState::Stopped;
                context.response.result_code = 0;
                Ok(())
            }

            // Command 3: AppendAudioOutBuffer(buffer_ptr: u64, buffer_size: u64, data_tag: u64)
            3 => {
                let ptr = context.request.read_u64(0).unwrap_or(0);
                let size = context.request.read_u64(8).unwrap_or(0);
                let tag = context.request.read_u64(16).unwrap_or(0);

                let sample_data = if context.request.raw_data.len() > 24 {
                    context.request.raw_data[24..].to_vec()
                } else {
                    vec![0u8; size as usize]
                };

                let buf = AudioOutBuffer {
                    buffer_ptr: ptr,
                    buffer_size: size,
                    data_tag: tag,
                    sample_data,
                };

                self.queue_buffer(buf);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 4: RegisterBufferEvent() -> out_event_handle
            4 => {
                context.spawned_events.push(self.buffer_event.clone());
                context.response.result_code = 0;
                Ok(())
            }

            // Command 5: GetReleasedAudioOutBuffers() -> count: u32, tags: [u64]
            5 => {
                self.poll_released_buffers();

                let mut released = self.released_buffers.lock().unwrap();
                let count = released.len() as u32;
                context.response.push_u32(count);

                for &tag in released.iter() {
                    context.response.push_u64(tag);
                }
                released.clear();

                context.response.result_code = 0;
                Ok(())
            }

            // Command 6: ContainsAudioOutBuffer(data_tag: u64) -> bool
            6 => {
                let tag = context.request.read_u64(0).unwrap_or(0);
                let queued = self.queued_buffers.lock().unwrap();
                let contains = queued.iter().any(|b| b.data_tag == tag);

                context.response.push_u32(if contains { 1 } else { 0 });
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

/// Service Manager factory handler for `audout:u` and `audout:a`.
pub struct AudioOutService {
    dsp: Arc<Mutex<AudioDsp>>,
}

impl Default for AudioOutService {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutService {
    pub fn new() -> Self {
        Self {
            dsp: Arc::new(Mutex::new(AudioDsp::new())),
        }
    }

    pub fn with_dsp(dsp: Arc<Mutex<AudioDsp>>) -> Self {
        Self { dsp }
    }
}

impl ServiceHandler for AudioOutService {
    fn service_name(&self) -> &str {
        "audout:u"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: ListAudioOuts() -> names: str
            0 => {
                let names = b"AudioTvOutput\0AudioBuiltInSpeaker\0AudioHeadphoneOutput\0";
                context.response.push_u32(names.len() as u32);
                context.response.push_bytes(names);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: OpenAudioOut(sample_rate: u32, channel_count: u16, name: str)
            //   -> sample_rate: u32, channel_count: u32, sample_format: u32, state: u32, session_handle, event_handle
            1 => {
                let req_rate = context.request.read_u32(0).unwrap_or(STANDARD_SAMPLE_RATE);
                let sample_rate = if req_rate == 0 { STANDARD_SAMPLE_RATE } else { req_rate };
                let channel_count = 2u32;
                let sample_format = 2u32; // PcmInt16
                let state = AudioOutState::Stopped as u32;

                let session = Arc::new(IAudioOutSession::new(self.dsp.clone(), 0));

                // Return session handle and buffer event handle
                context.spawned_sessions.push(("IAudioOut".into(), session.clone()));
                context.spawned_events.push(session.buffer_event.clone());

                context.response.push_u32(sample_rate);
                context.response.push_u32(channel_count);
                context.response.push_u32(sample_format);
                context.response.push_u32(state);
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
    fn test_audout_service_list_and_open() {
        let service = AudioOutService::new();

        // 1. ListAudioOuts
        let req_list = CmifRequest::new(0, vec![]);
        let mut ctx_list = IpcContext::new(req_list);
        service.dispatch(&mut ctx_list).unwrap();

        assert_eq!(ctx_list.response.result_code, 0);
        let list_str = String::from_utf8_lossy(&ctx_list.response.raw_data);
        assert!(list_str.contains("AudioTvOutput"));

        // 2. OpenAudioOut
        let req_open = CmifRequest::new(1, vec![0x80, 0xBB, 0x00, 0x00]); // 48000
        let mut ctx_open = IpcContext::new(req_open);
        service.dispatch(&mut ctx_open).unwrap();

        assert_eq!(ctx_open.response.result_code, 0);
        assert_eq!(ctx_open.spawned_sessions.len(), 1);
        assert_eq!(ctx_open.spawned_events.len(), 1);
    }

    #[test]
    fn test_iaudio_out_session_playback_lifecycle() {
        let dsp = Arc::new(Mutex::new(AudioDsp::new()));
        let session = IAudioOutSession::new(dsp.clone(), 0);

        // Check initial state (Stopped = 1)
        let req_state = CmifRequest::new(0, vec![]);
        let mut ctx = IpcContext::new(req_state);
        session.dispatch(&mut ctx).unwrap();
        assert_eq!(u32::from_le_bytes(ctx.response.raw_data[..4].try_into().unwrap()), 1);

        // StartAudioOut
        let req_start = CmifRequest::new(1, vec![]);
        let mut ctx_start = IpcContext::new(req_start);
        session.dispatch(&mut ctx_start).unwrap();
        assert_eq!(ctx_start.response.result_code, 0);

        // AppendAudioOutBuffer
        let mut append_data = Vec::new();
        append_data.extend_from_slice(&0x1000u64.to_le_bytes()); // ptr
        append_data.extend_from_slice(&320u64.to_le_bytes());  // size (80 frames stereo)
        append_data.extend_from_slice(&0x55AA55AAu64.to_le_bytes()); // tag
        append_data.extend_from_slice(&vec![100u8; 320]); // sample bytes

        let req_append = CmifRequest::new(3, append_data);
        let mut ctx_append = IpcContext::new(req_append);
        session.dispatch(&mut ctx_append).unwrap();
        assert_eq!(ctx_append.response.result_code, 0);

        // ContainsAudioOutBuffer should return 1
        let req_contains = CmifRequest::new(6, 0x55AA55AAu64.to_le_bytes().to_vec());
        let mut ctx_contains = IpcContext::new(req_contains);
        session.dispatch(&mut ctx_contains).unwrap();
        assert_eq!(u32::from_le_bytes(ctx_contains.response.raw_data[..4].try_into().unwrap()), 1);

        // Render audio in DSP to complete the buffer
        dsp.lock().unwrap().process_frame(80);

        // GetReleasedAudioOutBuffers
        let req_released = CmifRequest::new(5, vec![]);
        let mut ctx_released = IpcContext::new(req_released);
        session.dispatch(&mut ctx_released).unwrap();
        assert_eq!(ctx_released.response.result_code, 0);

        let count = u32::from_le_bytes(ctx_released.response.raw_data[..4].try_into().unwrap());
        assert_eq!(count, 1);
        let tag = u64::from_le_bytes(ctx_released.response.raw_data[4..12].try_into().unwrap());
        assert_eq!(tag, 0x55AA55AA);
    }
}
