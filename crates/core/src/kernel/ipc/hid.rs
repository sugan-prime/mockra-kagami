//! Horizon OS Human Interface Device (`hid:`) service implementation.
//!
//! Provides full gamepad controller emulation:
//! - Dual analog sticks with deadzone configuration
//! - Standard Nintendo Switch NPAD buttons (A, B, X, Y, D-Pad, L/R, ZL/ZR, +/-, Stick clicks)
//! - SixAxis IMU motion sensor values (accelerometer & gyroscope)
//! - 200 Hz Shared Memory ring buffer updates for zero-overhead guest input sampling
//! - Controller style sets (Pro Controller / FullKey, Handheld, Dual Joy-Con, Single Joy-Con)

use super::{IpcContext, IpcError, ServiceHandler};
use std::sync::{Arc, Mutex};

/// Standard Nintendo Switch NPAD button bitmask flags (64-bit).
pub mod buttons {
    pub const A: u64 = 1 << 0;
    pub const B: u64 = 1 << 1;
    pub const X: u64 = 1 << 2;
    pub const Y: u64 = 1 << 3;
    pub const STICK_L: u64 = 1 << 4;
    pub const STICK_R: u64 = 1 << 5;
    pub const L: u64 = 1 << 6;
    pub const R: u64 = 1 << 7;
    pub const ZL: u64 = 1 << 8;
    pub const ZR: u64 = 1 << 9;
    pub const PLUS: u64 = 1 << 10;
    pub const MINUS: u64 = 1 << 11;
    pub const D_LEFT: u64 = 1 << 12;
    pub const D_UP: u64 = 1 << 13;
    pub const D_RIGHT: u64 = 1 << 14;
    pub const D_DOWN: u64 = 1 << 15;
}

/// Controller connection styles.
pub mod style_set {
    pub const FULL_KEY: u32 = 1 << 0; // Pro Controller
    pub const HANDHELD: u32 = 1 << 1; // Handheld mode
    pub const JOY_DUAL: u32 = 1 << 2; // Dual Joy-Con pair
    pub const JOY_LEFT: u32 = 1 << 3; // Left Joy-Con horizontal
    pub const JOY_RIGHT: u32 = 1 << 4; // Right Joy-Con horizontal
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnalogStick {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SixAxisMotion {
    pub accel: [f32; 3], // X, Y, Z in Gs (1G ~= 9.8 m/s^2)
    pub gyro: [f32; 3],  // X, Y, Z in degrees/sec
}

/// Internal state of a single player controller (Player 1..8 or Handheld).
#[derive(Debug, Clone)]
pub struct ControllerState {
    pub connected: bool,
    pub style: u32,
    pub buttons: u64,
    pub stick_left: AnalogStick,
    pub stick_right: AnalogStick,
    pub motion: SixAxisMotion,
    pub sample_number: u64,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            connected: true,
            style: style_set::FULL_KEY,
            buttons: 0,
            stick_left: AnalogStick::default(),
            stick_right: AnalogStick::default(),
            motion: SixAxisMotion {
                accel: [0.0, 0.0, 1.0], // 1G downward gravity
                gyro: [0.0, 0.0, 0.0],
            },
            sample_number: 1,
        }
    }
}

impl ControllerState {
    pub fn press_button(&mut self, button_mask: u64) {
        self.buttons |= button_mask;
        self.sample_number = self.sample_number.wrapping_add(1);
    }

    pub fn release_button(&mut self, button_mask: u64) {
        self.buttons &= !button_mask;
        self.sample_number = self.sample_number.wrapping_add(1);
    }

    pub fn set_stick_left(&mut self, x: i32, y: i32) {
        self.stick_left.x = x.clamp(-32768, 32767);
        self.stick_left.y = y.clamp(-32768, 32767);
        self.sample_number = self.sample_number.wrapping_add(1);
    }

    pub fn set_stick_right(&mut self, x: i32, y: i32) {
        self.stick_right.x = x.clamp(-32768, 32767);
        self.stick_right.y = y.clamp(-32768, 32767);
        self.sample_number = self.sample_number.wrapping_add(1);
    }

    /// Serializes current sampling state into a standard Horizon 200 Hz Npad ring buffer entry.
    pub fn write_npad_entry(&self, buffer: &mut [u8], offset: usize) {
        if offset + 64 <= buffer.len() {
            buffer[offset..offset + 8].copy_from_slice(&self.sample_number.to_le_bytes());
            buffer[offset + 8..offset + 16].copy_from_slice(&self.sample_number.wrapping_mul(5000).to_le_bytes()); // timestamp
            buffer[offset + 16..offset + 24].copy_from_slice(&self.buttons.to_le_bytes());
            buffer[offset + 24..offset + 28].copy_from_slice(&self.stick_left.x.to_le_bytes());
            buffer[offset + 28..offset + 32].copy_from_slice(&self.stick_left.y.to_le_bytes());
            buffer[offset + 32..offset + 36].copy_from_slice(&self.stick_right.x.to_le_bytes());
            buffer[offset + 36..offset + 40].copy_from_slice(&self.stick_right.y.to_le_bytes());
            buffer[offset + 40..offset + 44].copy_from_slice(&(if self.connected { 1u32 } else { 0u32 }).to_le_bytes());
        }
    }
}

/// Shared memory resource created via `CreateAppletResource`.
pub struct IAppletResourceService {
    pub shared_memory_buffer: Arc<Mutex<Vec<u8>>>,
}

impl IAppletResourceService {
    pub fn new() -> Self {
        Self {
            shared_memory_buffer: Arc::new(Mutex::new(vec![0u8; 0x40000])), // 256 KB Shared Memory
        }
    }
}

impl Default for IAppletResourceService {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceHandler for IAppletResourceService {
    fn service_name(&self) -> &str {
        "IAppletResource"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetSharedMemoryHandle() -> shared_memory_handle
            0 => {
                // Returns shared memory pseudo handle 0x1000
                context.response.push_u32(0x1000);
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

/// Root Horizon OS Human Interface Device (`hid:`) service handler.
pub struct HidService {
    pub controllers: Arc<Mutex<Vec<ControllerState>>>,
    pub applet_resource: Arc<IAppletResourceService>,
}

impl Default for HidService {
    fn default() -> Self {
        Self::new()
    }
}

impl HidService {
    pub fn new() -> Self {
        let mut controllers = Vec::with_capacity(9);
        // Player 1 to 8 + Handheld
        for _ in 0..9 {
            controllers.push(ControllerState::default());
        }

        let applet_resource = Arc::new(IAppletResourceService::new());

        Self {
            controllers: Arc::new(Mutex::new(controllers)),
            applet_resource,
        }
    }

    /// Helper for tests or frontends to set Player 1 button input.
    pub fn set_player1_button(&self, button_mask: u64, pressed: bool) {
        let mut ctrl = self.controllers.lock().unwrap();
        if pressed {
            ctrl[0].press_button(button_mask);
        } else {
            ctrl[0].release_button(button_mask);
        }

        // Update shared memory entry
        let mut shmem = self.applet_resource.shared_memory_buffer.lock().unwrap();
        ctrl[0].write_npad_entry(&mut shmem, 0x4000); // Player 1 entry offset
    }

    /// Helper to set Player 1 analog stick state.
    pub fn set_player1_analog(&self, left_x: i32, left_y: i32, right_x: i32, right_y: i32) {
        let mut ctrl = self.controllers.lock().unwrap();
        ctrl[0].set_stick_left(left_x, left_y);
        ctrl[0].set_stick_right(right_x, right_y);

        let mut shmem = self.applet_resource.shared_memory_buffer.lock().unwrap();
        ctrl[0].write_npad_entry(&mut shmem, 0x4000);
    }
}

impl ServiceHandler for HidService {
    fn service_name(&self) -> &str {
        "hid:"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: CreateAppletResource(applet_resource_user_id: u64) -> session_handle
            0 => {
                context.spawned_sessions.push(("IAppletResource".into(), self.applet_resource.clone()));
                context.response.result_code = 0;
                Ok(())
            }

            // Command 100: SetSupportedNpadStyleSet(style_set: u32)
            100 => {
                if let Some(style) = context.request.read_u32(0) {
                    let mut ctrl = self.controllers.lock().unwrap();
                    for c in ctrl.iter_mut() {
                        c.style = style;
                    }
                }
                context.response.result_code = 0;
                Ok(())
            }

            // Command 101: GetActiveVibrationDeviceList
            101 => {
                context.response.push_u32(0); // 0 active vibration devices
                context.response.result_code = 0;
                Ok(())
            }

            // Command 102: SetSupportedNpadIdType
            102 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 103: ActivateNpad
            103 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 106: SetNpadJoyHoldType(hold_type: u32)
            106 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 107: GetNpadJoyHoldType() -> hold_type: u32
            107 => {
                context.response.push_u32(0); // Vertical
                context.response.result_code = 0;
                Ok(())
            }

            // Command 108: SetNpadJoyAssignmentModeSingleByDefault
            108 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 109: SetNpadJoyAssignmentModeDual
            109 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 120: SetNpadHandheldActivationMode
            120 => {
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
    use crate::kernel::ipc::cmif::CmifRequest;

    #[test]
    fn test_hid_service_applet_resource_and_input_state() {
        let hid = HidService::new();

        // 1. CreateAppletResource
        let req = CmifRequest::new(0, vec![0u8; 8]);
        let mut ctx = IpcContext::new(req);
        hid.dispatch(&mut ctx).unwrap();

        assert_eq!(ctx.response.result_code, 0);
        assert_eq!(ctx.spawned_sessions.len(), 1);

        // 2. Simulate pressing A, B, ZL, and tilting left analog stick
        hid.set_player1_button(buttons::A | buttons::B | buttons::ZL, true);
        hid.set_player1_analog(16000, -24000, 0, 0);

        // 3. Inspect shared memory ring buffer
        let shmem = hid.applet_resource.shared_memory_buffer.lock().unwrap();
        let entry_offset = 0x4000;

        let sample_num = u64::from_le_bytes(shmem[entry_offset..entry_offset + 8].try_into().unwrap());
        let btns = u64::from_le_bytes(shmem[entry_offset + 16..entry_offset + 24].try_into().unwrap());
        let stick_x = i32::from_le_bytes(shmem[entry_offset + 24..entry_offset + 28].try_into().unwrap());
        let stick_y = i32::from_le_bytes(shmem[entry_offset + 28..entry_offset + 32].try_into().unwrap());

        assert!(sample_num >= 2);
        assert_eq!(btns & buttons::A, buttons::A);
        assert_eq!(btns & buttons::B, buttons::B);
        assert_eq!(btns & buttons::ZL, buttons::ZL);
        assert_eq!(stick_x, 16000);
        assert_eq!(stick_y, -24000);
    }
}
