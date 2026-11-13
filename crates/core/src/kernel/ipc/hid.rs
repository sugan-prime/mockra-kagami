//! Horizon OS Human Interface Device (`hid:`) service implementation.
//!
//! Manages controller state, Joy-Cons, Pro Controllers, touch screen, and gamepad inputs.

use super::{IpcContext, IpcError, ServiceHandler};

pub struct HidService;

impl Default for HidService {
    fn default() -> Self {
        Self::new()
    }
}

impl HidService {
    pub fn new() -> Self {
        Self
    }
}

impl ServiceHandler for HidService {
    fn service_name(&self) -> &str {
        "hid:"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: CreateAppletResource(applet_resource_user_id: u64)
            0 => {
                context.response.result_code = 0; // Success
                Ok(())
            }

            // Command 100: SetSupportedNpadStyleSet(style_set: u32)
            100 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 101: GetActiveVibrationDeviceList
            101 => {
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

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ipc::cmif::CmifRequest;

    #[test]
    fn test_hid_service_activate_npad() {
        let hid = HidService::new();
        let mut req_bytes = vec![0u8; 32];
        req_bytes[0x00..0x04].copy_from_slice(b"SFCI");
        req_bytes[0x08..0x0C].copy_from_slice(&103u32.to_le_bytes()); // Cmd 103: ActivateNpad

        let req = CmifRequest::parse(&req_bytes).unwrap();
        let mut ctx = IpcContext::new(req);

        hid.dispatch(&mut ctx).unwrap();
        assert_eq!(ctx.response.result_code, 0);
    }
}
