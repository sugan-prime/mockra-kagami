//! Horizon OS Account Management Services (`acc:u0`, `acc:u1`) implementation.
//!
//! Manages Nintendo Switch user profiles, account identification, and saved credentials.

use super::{IpcContext, IpcError, ServiceHandler};
use std::sync::Arc;

pub const DEFAULT_USER_ID: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

pub struct IProfileSession;

impl ServiceHandler for IProfileSession {
    fn service_name(&self) -> &str {
        "IProfile"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: Get() -> user_data
            0 | 1 => {
                // Return UserData: UserId (16 bytes) + Nickname ("Player\0")
                context.response.push_bytes(&DEFAULT_USER_ID);
                let mut name_buf = [0u8; 32];
                name_buf[..6].copy_from_slice(b"Player");
                context.response.push_bytes(&name_buf);
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

pub struct AccountService {
    profile: Arc<IProfileSession>,
}

impl Default for AccountService {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountService {
    pub fn new() -> Self {
        Self {
            profile: Arc::new(IProfileSession),
        }
    }
}

impl ServiceHandler for AccountService {
    fn service_name(&self) -> &str {
        "acc:u0"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetUserCount() -> count: i32
            0 => {
                context.response.push_u32(1);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: GetUserExistence(user_id) -> bool
            1 => {
                context.response.push_bytes(&[1]); // exists
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: ListAllUsers() -> [user_id]
            2 => {
                context.response.push_bytes(&DEFAULT_USER_ID);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 3: OpenUser(user_id) -> session_handle
            3 => {
                context.spawned_sessions.push(("IProfile".into(), self.profile.clone()));
                context.response.result_code = 0;
                Ok(())
            }

            // Command 4: GetLastOpenedUser() -> user_id
            4 => {
                context.response.push_bytes(&DEFAULT_USER_ID);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 50: InitializeApplicationInfo
            50 | 51 => {
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
    fn test_account_service_user_profile() {
        let acc = AccountService::new();

        // GetUserCount
        let req_count = CmifRequest::new(0, vec![]);
        let mut ctx_count = IpcContext::new(req_count);
        acc.dispatch(&mut ctx_count).unwrap();

        assert_eq!(ctx_count.response.result_code, 0);
        let count = u32::from_le_bytes(ctx_count.response.raw_data[..4].try_into().unwrap());
        assert_eq!(count, 1);

        // OpenUser
        let req_open = CmifRequest::new(3, DEFAULT_USER_ID.to_vec());
        let mut ctx_open = IpcContext::new(req_open);
        acc.dispatch(&mut ctx_open).unwrap();

        assert_eq!(ctx_open.spawned_sessions.len(), 1);
        let profile_sess = ctx_open.spawned_sessions[0].1.clone();

        // Get Profile Name
        let req_prof = CmifRequest::new(0, vec![]);
        let mut ctx_prof = IpcContext::new(req_prof);
        profile_sess.dispatch(&mut ctx_prof).unwrap();

        let prof_str = String::from_utf8_lossy(&ctx_prof.response.raw_data[16..]);
        assert!(prof_str.starts_with("Player"));
    }
}
