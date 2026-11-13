//! Horizon OS Time Service (`time:`) implementation.
//!
//! Provides system clock and POSIX wall-clock timestamps to guest titles.

use super::{IpcContext, IpcError, ServiceHandler};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TimeService;

impl Default for TimeService {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeService {
    pub fn new() -> Self {
        Self
    }

    /// Returns the current Unix epoch time in seconds.
    pub fn current_posix_time() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1700000000)
    }
}

impl ServiceHandler for TimeService {
    fn service_name(&self) -> &str {
        "time:"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetStandardUserSystemClock / GetCurrentTime
            0 => {
                let current_time = Self::current_posix_time();
                context.response.push_u64(current_time);
                context.response.result_code = 0; // Success
                Ok(())
            }

            // Command 1: GetStandardNetworkSystemClock
            1 => {
                let current_time = Self::current_posix_time();
                context.response.push_u64(current_time);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: GetStandardSteadyClock
            2 => {
                let current_time = Self::current_posix_time();
                context.response.push_u64(current_time);
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
    fn test_time_service_get_current_time() {
        let time_svc = TimeService::new();
        let mut req_bytes = vec![0u8; 32];
        req_bytes[0x00..0x04].copy_from_slice(b"SFCI");
        req_bytes[0x08..0x0C].copy_from_slice(&0u32.to_le_bytes()); // Cmd 0

        let req = CmifRequest::parse(&req_bytes).unwrap();
        let mut ctx = IpcContext::new(req);

        time_svc.dispatch(&mut ctx).unwrap();
        assert_eq!(ctx.response.result_code, 0);

        // Verify that timestamp returned is valid
        let ts_bytes: [u8; 8] = ctx.response.raw_data[0..8].try_into().unwrap();
        let ts = u64::from_le_bytes(ts_bytes);
        assert!(ts > 1600000000);
    }
}
