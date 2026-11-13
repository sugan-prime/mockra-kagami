//! Audio Subsystem for Nintendo Switch emulation.
//!
//! Provides the Horizon OS Audio Architecture:
//! - DSP Voice Mixing & Resampling Engine (`AudioDsp`)
//! - Low-level Audio Out Service (`audout:u`, `audout:a`)
//! - High-level DSP Audio Renderer Service (`audren:u`, `audren:a`)

pub mod audout;
pub mod audren;
pub mod dsp;

pub use audout::{AudioOutBuffer, AudioOutService, AudioOutState, IAudioOutSession};
pub use audren::{
    AudioRendererConfig, AudioRendererService, AudioRendererState,
    IAudioDeviceSession, IAudioRendererSession,
};
pub use dsp::{
    AudioDsp, BiquadFilter, SampleFormat, Voice, VoiceState, WaveBuffer,
    DEFAULT_CHANNEL_COUNT, MAX_MIX_BUFFERS, MAX_VOICES, STANDARD_SAMPLE_RATE,
};
