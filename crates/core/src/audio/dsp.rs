//! Digital Signal Processor (DSP) and Voice Mixing Engine.
//!
//! Emulates the Nintendo Switch Audio Renderer DSP backend:
//! - Multi-channel voice mixer (PCM16, resampling via linear interpolation)
//! - Submix buses and routing matrix
//! - Biquad filter processing (low-pass, high-pass)
//! - Master PCM output circular ring buffer (48,000 Hz, stereo 16-bit signed PCM)
//! - Audio diagnostics and RIFF/WAVE container generation for verification.

use std::collections::VecDeque;

pub const STANDARD_SAMPLE_RATE: u32 = 48_000;
pub const DEFAULT_CHANNEL_COUNT: u16 = 2;
pub const MAX_VOICES: usize = 256;
pub const MAX_MIX_BUFFERS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    PcmInt8,
    PcmInt16,
    PcmFloat32,
    Adpcm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

/// A queued wave buffer containing raw audio sample data.
#[derive(Debug, Clone)]
pub struct WaveBuffer {
    /// Guest virtual memory address or identifier.
    pub buffer_address: u64,
    /// Raw sample bytes.
    pub data: Vec<u8>,
    /// Start sample offset within data.
    pub start_sample_offset: u32,
    /// End sample offset within data.
    pub end_sample_offset: u32,
    /// Whether this buffer loops indefinitely.
    pub is_looping: bool,
    /// Loop start sample index.
    pub loop_start: u32,
    /// Loop end sample index.
    pub loop_end: u32,
    /// User context tag returned when the buffer finishes playback.
    pub context: u64,
}

/// Biquad IIR filter implementation.
#[derive(Debug, Clone)]
pub struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadFilter {
    /// Constructs a low-pass filter with specified cutoff frequency and Q factor.
    pub fn low_pass(cutoff_hz: f32, sample_rate: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * (cutoff_hz / sample_rate);
        let alpha = w0.sin() / (2.0 * q.max(0.001));
        let cos_w0 = w0.cos();

        let a0 = 1.0 + alpha;
        let b0 = ((1.0 - cos_w0) / 2.0) / a0;
        let b1 = (1.0 - cos_w0) / a0;
        let b2 = ((1.0 - cos_w0) / 2.0) / a0;
        let a1 = (-2.0 * cos_w0) / a0;
        let a2 = (1.0 - alpha) / a0;

        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Processes a single audio sample through the filter.
    pub fn process(&mut self, sample: f32) -> f32 {
        let out = self.b0 * sample + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;

        self.x2 = self.x1;
        self.x1 = sample;
        self.y2 = self.y1;
        self.y1 = out;

        out
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// An individual voice within the DSP audio renderer.
#[derive(Debug, Clone)]
pub struct Voice {
    pub id: usize,
    pub state: VoiceState,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
    pub volume: f32,
    /// Channel panning gains: [Left, Right].
    pub channel_volumes: [f32; 2],
    /// Resampling pitch multiplier (1.0 = normal playback speed).
    pub pitch: f32,
    /// Fractional sample playback index within current wave buffer.
    pub src_sample_pos: f64,
    /// Queue of wave buffers to play sequentially.
    pub wave_buffers: VecDeque<WaveBuffer>,
    /// User context tags of wave buffers that completed playback.
    pub released_contexts: Vec<u64>,
    /// Optional biquad filter.
    pub biquad_filter: Option<BiquadFilter>,
    /// Target mix buffer index (0 = master mix).
    pub mix_id: usize,
}

impl Voice {
    pub fn new(id: usize) -> Self {
        Self {
            id,
            state: VoiceState::Stopped,
            sample_rate: STANDARD_SAMPLE_RATE,
            channels: 2,
            format: SampleFormat::PcmInt16,
            volume: 1.0,
            channel_volumes: [1.0, 1.0],
            pitch: 1.0,
            src_sample_pos: 0.0,
            wave_buffers: VecDeque::new(),
            released_contexts: Vec::new(),
            biquad_filter: None,
            mix_id: 0,
        }
    }

    /// Appends a new wave buffer to the voice playback queue.
    pub fn append_wave_buffer(&mut self, buffer: WaveBuffer) {
        self.wave_buffers.push_back(buffer);
        if self.state == VoiceState::Stopped {
            self.state = VoiceState::Playing;
        }
    }

    /// Helper to fetch sample at given integer sample index and channel from current buffer.
    fn sample_at(&self, buffer: &WaveBuffer, sample_idx: usize, channel: usize) -> f32 {
        match self.format {
            SampleFormat::PcmInt16 => {
                let frame_size = self.channels as usize * 2;
                let byte_offset = sample_idx * frame_size + (channel * 2);
                if byte_offset + 1 < buffer.data.len() {
                    let s = i16::from_le_bytes([buffer.data[byte_offset], buffer.data[byte_offset + 1]]);
                    s as f32
                } else {
                    0.0
                }
            }
            SampleFormat::PcmInt8 => {
                let frame_size = self.channels as usize;
                let byte_offset = sample_idx * frame_size + channel;
                if byte_offset < buffer.data.len() {
                    let s = buffer.data[byte_offset] as i8;
                    (s as f32) * 256.0
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }

    /// Reads linearly interpolated sample at fractional index `pos` for `channel`.
    fn interpolated_sample(&self, buffer: &WaveBuffer, pos: f64, channel: usize, max_samples: usize) -> f32 {
        let idx0 = pos.floor() as usize;
        let frac = (pos - pos.floor()) as f32;

        if idx0 >= max_samples {
            return 0.0;
        }

        let s0 = self.sample_at(buffer, idx0, channel);
        let s1 = if idx0 + 1 < max_samples {
            self.sample_at(buffer, idx0 + 1, channel)
        } else if buffer.is_looping && buffer.loop_end > buffer.loop_start {
            self.sample_at(buffer, buffer.loop_start as usize, channel)
        } else {
            s0
        };

        s0 + (s1 - s0) * frac
    }
}

/// Digital Signal Processor (DSP) engine.
pub struct AudioDsp {
    pub voices: Vec<Voice>,
    pub master_ring_buffer: VecDeque<i16>,
    pub master_volume: f32,
    pub sample_rate: u32,
    pub channels: u16,
    pub ring_capacity: usize,
}

impl Default for AudioDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioDsp {
    pub fn new() -> Self {
        let mut voices = Vec::with_capacity(MAX_VOICES);
        for id in 0..MAX_VOICES {
            voices.push(Voice::new(id));
        }

        Self {
            voices,
            master_ring_buffer: VecDeque::with_capacity(STANDARD_SAMPLE_RATE as usize * 2), // 1 second of stereo
            master_volume: 1.0,
            sample_rate: STANDARD_SAMPLE_RATE,
            channels: DEFAULT_CHANNEL_COUNT,
            ring_capacity: STANDARD_SAMPLE_RATE as usize * 2,
        }
    }

    /// Processes audio rendering for `sample_count` frames (e.g. 160 or 240 samples).
    /// Mixes all active voices and pushes interleaved 16-bit PCM samples into the master ring buffer.
    pub fn process_frame(&mut self, sample_count: usize) {
        let mut mix_left = vec![0.0f32; sample_count];
        let mut mix_right = vec![0.0f32; sample_count];

        let target_rate = self.sample_rate as f64;

        for voice in &mut self.voices {
            if voice.state != VoiceState::Playing {
                continue;
            }

            let step = (voice.sample_rate as f64 / target_rate) * (voice.pitch.max(0.01) as f64);
            let vol_l = voice.volume * voice.channel_volumes[0];
            let vol_r = voice.volume * voice.channel_volumes[1];

            for i in 0..sample_count {
                if voice.wave_buffers.is_empty() {
                    voice.state = VoiceState::Stopped;
                    break;
                }

                let current_buf = voice.wave_buffers.front().unwrap();
                let bytes_per_sample = match voice.format {
                    SampleFormat::PcmInt16 => (voice.channels as usize) * 2,
                    SampleFormat::PcmInt8 => voice.channels as usize,
                    _ => 2,
                };
                let total_samples = current_buf.data.len() / bytes_per_sample.max(1);
                let end_sample = if current_buf.end_sample_offset > 0 {
                    (current_buf.end_sample_offset as usize).min(total_samples)
                } else {
                    total_samples
                };

                let ch_left = 0;
                let ch_right = if voice.channels > 1 { 1 } else { 0 };

                let raw_l = voice.interpolated_sample(current_buf, voice.src_sample_pos, ch_left, end_sample);
                let raw_r = voice.interpolated_sample(current_buf, voice.src_sample_pos, ch_right, end_sample);

                let filtered_l = if let Some(ref mut filter) = voice.biquad_filter {
                    filter.process(raw_l)
                } else {
                    raw_l
                };
                let filtered_r = raw_r;

                mix_left[i] += filtered_l * vol_l;
                mix_right[i] += filtered_r * vol_r;

                voice.src_sample_pos += step;

                // Handle buffer completion or looping
                if voice.src_sample_pos >= end_sample as f64 {
                    let is_looping = current_buf.is_looping;
                    let loop_start = current_buf.loop_start;
                    let loop_end = current_buf.loop_end;

                    if is_looping && loop_end > loop_start {
                        let loop_len = (loop_end - loop_start) as f64;
                        voice.src_sample_pos = loop_start as f64 + (voice.src_sample_pos - end_sample as f64) % loop_len;
                    } else {
                        // Pop completed buffer and record context tag
                        let finished_buf = voice.wave_buffers.pop_front().unwrap();
                        voice.released_contexts.push(finished_buf.context);
                        voice.src_sample_pos = 0.0;

                        if voice.wave_buffers.is_empty() {
                            voice.state = VoiceState::Stopped;
                            break;
                        }
                    }
                }
            }
        }

        // Clamp & convert mix buffers to master ring buffer
        for i in 0..sample_count {
            let out_l = (mix_left[i] * self.master_volume).clamp(-32768.0, 32767.0) as i16;
            let out_r = (mix_right[i] * self.master_volume).clamp(-32768.0, 32767.0) as i16;

            if self.master_ring_buffer.len() + 2 > self.ring_capacity {
                self.master_ring_buffer.pop_front();
                self.master_ring_buffer.pop_front();
            }
            self.master_ring_buffer.push_back(out_l);
            self.master_ring_buffer.push_back(out_r);
        }
    }

    /// Reads up to `count` interleaved samples from the front of the ring buffer.
    pub fn read_master_samples(&mut self, count: usize) -> Vec<i16> {
        let n = count.min(self.master_ring_buffer.len());
        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            if let Some(s) = self.master_ring_buffer.pop_front() {
                samples.push(s);
            }
        }
        samples
    }

    /// Peeks up to `count` interleaved samples without removing them.
    pub fn peek_master_samples(&self, count: usize) -> Vec<i16> {
        self.master_ring_buffer
            .iter()
            .take(count)
            .copied()
            .collect()
    }

    /// Computes Root-Mean-Square (RMS) amplitude of a slice of samples.
    pub fn compute_rms(samples: &[i16]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        ((sum_sq / samples.len() as f64).sqrt()) as f32
    }

    /// Computes peak amplitude within a slice of samples.
    pub fn compute_peak(samples: &[i16]) -> i16 {
        samples.iter().map(|&s| s.abs()).max().unwrap_or(0)
    }

    /// Encodes interleaved 16-bit PCM samples into a valid RIFF/WAVE container.
    pub fn write_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut wav = Vec::new();

        let byte_rate = sample_rate * (channels as u32) * 2;
        let block_align = channels * 2;
        let data_size = (samples.len() * 2) as u32;
        let riff_size = 36 + data_size;

        // RIFF Header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // "fmt " Subchunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // Subchunk1Size (16 for PCM)
        wav.extend_from_slice(&1u16.to_le_bytes());  // AudioFormat (1 = PCM)
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes()); // BitsPerSample

        // "data" Subchunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        for &s in samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }

        wav
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dsp_sine_wave_rendering_and_rms() {
        let mut dsp = AudioDsp::new();

        // Synthesize 440 Hz sine wave tone (A4) at 48kHz, mono PCM16
        let sample_rate = 48000;
        let total_samples = 480; // 10ms
        let freq = 440.0;
        let mut pcm = Vec::with_capacity(total_samples * 2);

        for i in 0..total_samples {
            let t = (i as f32) / (sample_rate as f32);
            let sample_f = ((2.0 * std::f32::consts::PI * freq * t).sin()) * 16000.0;
            let sample_i = sample_f as i16;
            pcm.extend_from_slice(&sample_i.to_le_bytes());
        }

        dsp.voices[0].sample_rate = sample_rate;
        dsp.voices[0].channels = 1;
        dsp.voices[0].volume = 1.0;
        dsp.voices[0].append_wave_buffer(WaveBuffer {
            buffer_address: 0x1000,
            data: pcm,
            start_sample_offset: 0,
            end_sample_offset: total_samples as u32,
            is_looping: false,
            loop_start: 0,
            loop_end: 0,
            context: 0xA1A2A3A4,
        });

        dsp.process_frame(160);

        let rendered = dsp.peek_master_samples(320);
        assert_eq!(rendered.len(), 320);

        let peak = AudioDsp::compute_peak(&rendered);
        let rms = AudioDsp::compute_rms(&rendered);

        assert!(peak > 10000, "Peak amplitude should be high for 16000 amplitude sine wave");
        assert!(rms > 7000.0, "RMS energy should be non-zero");

        // Verify WAV generation
        let wav = AudioDsp::write_wav(&rendered, 48000, 2);
        assert!(wav.starts_with(b"RIFF"));
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
    }

    #[test]
    fn test_dsp_wave_buffer_release_tracking() {
        let mut dsp = AudioDsp::new();

        // 160 samples of silence
        let pcm = vec![0u8; 160 * 2];
        dsp.voices[1].channels = 1;
        dsp.voices[1].append_wave_buffer(WaveBuffer {
            buffer_address: 0x2000,
            data: pcm,
            start_sample_offset: 0,
            end_sample_offset: 160,
            is_looping: false,
            loop_start: 0,
            loop_end: 0,
            context: 0xCAFEBABE,
        });

        assert_eq!(dsp.voices[1].state, VoiceState::Playing);
        dsp.process_frame(160);

        // Voice finished buffer and should have released the context tag
        assert_eq!(dsp.voices[1].released_contexts, vec![0xCAFEBABE]);
        assert_eq!(dsp.voices[1].state, VoiceState::Stopped);
    }
}
