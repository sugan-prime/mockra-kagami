//! Horizon OS Network Interface Manager (nifm) HLE Service.
//!
//! Emulates `nifm:u`, `nifm:a`, and `nifm:s`, handling network configuration queries,
//! internet connection status, and asynchronous network request lifecycle.

use super::{IpcContext, IpcError, ServiceHandler};
use crate::kernel::object::KEvent;
use std::sync::{Arc, Mutex};

/// IP configuration for guest network interfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpConfig {
    pub ip_address: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub default_gateway: [u8; 4],
    pub primary_dns: [u8; 4],
    pub secondary_dns: [u8; 4],
}

impl Default for IpConfig {
    fn default() -> Self {
        Self {
            ip_address: [192, 168, 1, 105],
            subnet_mask: [255, 255, 255, 0],
            default_gateway: [192, 168, 1, 1],
            primary_dns: [8, 8, 8, 8],
            secondary_dns: [8, 8, 4, 4],
        }
    }
}

/// Request state for `IRequest` sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    NotSubmitted = 0,
    OnWork = 1,
    Completed = 2,
    Error = 3,
}

/// `IRequest` session returned by `IGeneralService::CreateRequest`.
pub struct NetworkRequestSession {
    pub state: Mutex<RequestState>,
    pub completion_event: KEvent,
}

impl NetworkRequestSession {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(RequestState::NotSubmitted),
            completion_event: KEvent::new(false),
        }
    }
}

impl ServiceHandler for NetworkRequestSession {
    fn service_name(&self) -> &str {
        "IRequest"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Cmd 0: GetRequestState() -> state: u32
            0 => {
                let state = *self.state.lock().unwrap() as u32;
                context.response.push_u32(state);
                Ok(())
            }

            // Cmd 1: GetResult() -> result: u32
            1 => {
                context.response.push_u32(0); // Success
                Ok(())
            }

            // Cmd 2: GetSystemEventReadableHandle() -> handle
            2 => {
                context.spawned_events.push(self.completion_event.clone());
                Ok(())
            }

            // Cmd 3: Cancel()
            3 => {
                let mut state = self.state.lock().unwrap();
                *state = RequestState::NotSubmitted;
                Ok(())
            }

            // Cmd 4: Submit()
            4 => {
                let mut state = self.state.lock().unwrap();
                *state = RequestState::Completed;
                self.completion_event.signal();
                Ok(())
            }

            other => Err(IpcError::UnknownCommand(other)),
        }
    }
}

/// `IGeneralService` session returned by `nifm:u::CreateGeneralService`.
pub struct GeneralService {
    pub config: IpConfig,
}

impl GeneralService {
    pub fn new(config: IpConfig) -> Self {
        Self { config }
    }
}

impl ServiceHandler for GeneralService {
    fn service_name(&self) -> &str {
        "IGeneralService"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Cmd 1: GetClientId() -> client_id: u64
            1 => {
                context.response.push_u64(1);
                Ok(())
            }

            // Cmd 4: CreateRequest(requirement_preset: u32) -> IRequest session
            4 => {
                let req_session = Arc::new(NetworkRequestSession::new());
                context
                    .spawned_sessions
                    .push(("IRequest".to_string(), req_session));
                Ok(())
            }

            // Cmd 12: GetCurrentIpAddress() -> ip: u32
            12 => {
                let ip_u32 = u32::from_le_bytes(self.config.ip_address);
                context.response.push_u32(ip_u32);
                Ok(())
            }

            // Cmd 15: GetCurrentIpConfigInfo() -> IpConfigInfo
            15 => {
                context.response.push_bytes(&self.config.ip_address);
                context.response.push_bytes(&self.config.subnet_mask);
                context.response.push_bytes(&self.config.default_gateway);
                context.response.push_bytes(&self.config.primary_dns);
                context.response.push_bytes(&self.config.secondary_dns);
                Ok(())
            }

            // Cmd 18: IsWirelessCommunicationEnabled() -> enabled: u8
            18 => {
                context.response.push_bytes(&[1u8, 0, 0, 0]); // Enabled
                Ok(())
            }

            // Cmd 21: GetInternetConnectionStatus() -> InternetConnectionStatus
            // Layout:
            // +0x00: type (1 = Ethernet, 2 = Wi-Fi)
            // +0x01: wifi_strength (0..3)
            // +0x02: state (2 = Connected)
            // +0x03..0x08: reserved
            21 => {
                let status_payload = [
                    2u8, // Wi-Fi
                    3u8, // 3 bars
                    2u8, // Connected
                    0u8, 0u8, 0u8, 0u8, 0u8,
                ];
                context.response.push_bytes(&status_payload);
                Ok(())
            }

            other => Err(IpcError::UnknownCommand(other)),
        }
    }
}

/// Root Network Interface Manager Service (`nifm:u`, `nifm:a`, `nifm:s`).
pub struct NifmService {
    pub config: IpConfig,
}

impl Default for NifmService {
    fn default() -> Self {
        Self::new()
    }
}

impl NifmService {
    pub fn new() -> Self {
        Self {
            config: IpConfig::default(),
        }
    }

    pub fn with_ip_config(mut self, config: IpConfig) -> Self {
        self.config = config;
        self
    }
}

impl ServiceHandler for NifmService {
    fn service_name(&self) -> &str {
        "nifm:u"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Cmd 4: CreateGeneralService() -> IGeneralService
            // Cmd 5: CreateGeneralServiceOld() -> IGeneralService
            4 | 5 => {
                let general_service = Arc::new(GeneralService::new(self.config));
                context
                    .spawned_sessions
                    .push(("IGeneralService".to_string(), general_service));
                Ok(())
            }

            other => Err(IpcError::UnknownCommand(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ipc::cmif::CmifRequest;

    #[test]
    fn test_nifm_general_service_ip_and_status() {
        let nifm = NifmService::new();

        // 1. CreateGeneralService (Cmd 4)
        let req_buf = [
            b'S', b'F', b'C', b'I',
            0, 0, 0, 0,
            4, 0, 0, 0, // Cmd 4
            0, 0, 0, 0,
        ];
        let req = CmifRequest::parse(&req_buf).unwrap();
        let mut ctx = IpcContext::new(req);
        nifm.dispatch(&mut ctx).unwrap();

        assert_eq!(ctx.response.result_code, 0);
        assert_eq!(ctx.spawned_sessions.len(), 1);
        let general_svc = ctx.spawned_sessions[0].1.clone();

        // 2. GetCurrentIpAddress (Cmd 12)
        let ip_req_buf = [
            b'S', b'F', b'C', b'I',
            0, 0, 0, 0,
            12, 0, 0, 0, // Cmd 12
            0, 0, 0, 0,
        ];
        let mut ip_ctx = IpcContext::new(CmifRequest::parse(&ip_req_buf).unwrap());
        general_svc.dispatch(&mut ip_ctx).unwrap();
        assert_eq!(ip_ctx.response.result_code, 0);

        let ip_bytes: [u8; 4] = ip_ctx.response.raw_data[0..4].try_into().unwrap();
        assert_eq!(ip_bytes, [192, 168, 1, 105]);

        // 3. GetInternetConnectionStatus (Cmd 21)
        let status_req_buf = [
            b'S', b'F', b'C', b'I',
            0, 0, 0, 0,
            21, 0, 0, 0, // Cmd 21
            0, 0, 0, 0,
        ];
        let mut status_ctx = IpcContext::new(CmifRequest::parse(&status_req_buf).unwrap());
        general_svc.dispatch(&mut status_ctx).unwrap();
        assert_eq!(status_ctx.response.result_code, 0);
        assert_eq!(status_ctx.response.raw_data[0], 2); // Wi-Fi
        assert_eq!(status_ctx.response.raw_data[2], 2); // Connected
    }

    #[test]
    fn test_nifm_request_session_submit_lifecycle() {
        let general_svc = GeneralService::new(IpConfig::default());

        // 1. CreateRequest (Cmd 4)
        let req_buf = [
            b'S', b'F', b'C', b'I',
            0, 0, 0, 0,
            4, 0, 0, 0, // Cmd 4
            0, 0, 0, 0,
        ];
        let mut ctx = IpcContext::new(CmifRequest::parse(&req_buf).unwrap());
        general_svc.dispatch(&mut ctx).unwrap();
        let req_session = ctx.spawned_sessions[0].1.clone();

        // 2. GetSystemEventReadableHandle (Cmd 2)
        let event_req_buf = [
            b'S', b'F', b'C', b'I',
            0, 0, 0, 0,
            2, 0, 0, 0, // Cmd 2
            0, 0, 0, 0,
        ];
        let mut event_ctx = IpcContext::new(CmifRequest::parse(&event_req_buf).unwrap());
        req_session.dispatch(&mut event_ctx).unwrap();
        assert_eq!(event_ctx.spawned_events.len(), 1);
        let completion_event = &event_ctx.spawned_events[0];
        assert!(!completion_event.is_signaled());

        // 3. Submit (Cmd 4)
        let submit_req_buf = [
            b'S', b'F', b'C', b'I',
            0, 0, 0, 0,
            4, 0, 0, 0, // Cmd 4
            0, 0, 0, 0,
        ];
        let mut submit_ctx = IpcContext::new(CmifRequest::parse(&submit_req_buf).unwrap());
        req_session.dispatch(&mut submit_ctx).unwrap();

        // Verified signaled on completion
        assert!(completion_event.is_signaled());
    }
}
