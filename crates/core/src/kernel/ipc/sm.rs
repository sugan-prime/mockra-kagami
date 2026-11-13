//! Horizon OS Service Manager (`sm:`) implementation.
//!
//! The Service Manager is the root IPC gateway on Nintendo Switch. Userland
//! processes query `sm:` to acquire session handles to system services
//! like `time:`, `hid:`, `fsp-srv:`, etc.

use super::{IpcContext, IpcError, ServiceHandler};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type ServiceFactory = Box<dyn Fn() -> Arc<dyn ServiceHandler> + Send + Sync>;

pub struct ServiceManager {
    factories: Mutex<HashMap<String, ServiceFactory>>,
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceManager {
    pub fn new() -> Self {
        let sm = Self {
            factories: Mutex::new(HashMap::new()),
        };

        // Register default built-in HLE services
        sm.register_factory("time", || Arc::new(super::time::TimeService::new()));
        sm.register_factory("hid", || Arc::new(super::hid::HidService::new()));
        sm.register_factory("nvdrv", || Arc::new(crate::gpu::nvdrv::NvDrvService::new()));
        sm.register_factory("nvdrv:a", || Arc::new(crate::gpu::nvdrv::NvDrvService::new()));
        sm.register_factory("fsp-srv", || Arc::new(crate::fs::fsp_srv::FileSystemProxyService::new()));
        sm.register_factory("fsp-pr", || Arc::new(crate::fs::fsp_srv::FileSystemProxyService::new()));
        sm.register_factory("audout:u", || Arc::new(crate::audio::AudioOutService::new()));
        sm.register_factory("audout:a", || Arc::new(crate::audio::AudioOutService::new()));
        sm.register_factory("audout", || Arc::new(crate::audio::AudioOutService::new()));
        sm.register_factory("audren:u", || Arc::new(crate::audio::AudioRendererService::new()));
        sm.register_factory("audren:a", || Arc::new(crate::audio::AudioRendererService::new()));
        sm.register_factory("audren", || Arc::new(crate::audio::AudioRendererService::new()));

        sm
    }

    /// Registers a factory function for creating new service instances upon request.
    pub fn register_factory<F>(&self, name: impl Into<String>, factory: F)
    where
        F: Fn() -> Arc<dyn ServiceHandler> + Send + Sync + 'static,
    {
        self.factories
            .lock()
            .unwrap()
            .insert(name.into(), Box::new(factory));
    }
}

impl ServiceHandler for ServiceManager {
    fn service_name(&self) -> &str {
        "sm:"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: Initialize(reserved: u64)
            0 => {
                // Horizon OS accepts initialization handshake
                context.response.result_code = 0; // Success
                Ok(())
            }

            // Command 1: GetService(name: [u8; 8]) -> session_handle
            1 => {
                let service_name = context
                    .request
                    .read_string_8(0)
                    .ok_or(IpcError::InvalidPayload("Missing 8-byte service name"))?;

                let factories = self.factories.lock().unwrap();
                if let Some(factory) = factories.get(&service_name) {
                    let instance = factory();
                    context
                        .spawned_sessions
                        .push((service_name.clone(), instance));
                    context.response.result_code = 0; // Success
                    Ok(())
                } else {
                    Err(IpcError::ServiceNotFound(service_name))
                }
            }

            // Command 2: RegisterService
            2 => {
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
    fn test_sm_initialize() {
        let sm = ServiceManager::new();
        let mut req_bytes = vec![0u8; 32];
        req_bytes[0x00..0x04].copy_from_slice(b"SFCI");
        req_bytes[0x08..0x0C].copy_from_slice(&0u32.to_le_bytes()); // Cmd 0: Initialize

        let req = CmifRequest::parse(&req_bytes).unwrap();
        let mut ctx = IpcContext::new(req);

        sm.dispatch(&mut ctx).unwrap();
        assert_eq!(ctx.response.result_code, 0);
    }

    #[test]
    fn test_sm_get_service_time() {
        let sm = ServiceManager::new();
        let mut req_bytes = vec![0u8; 32];
        req_bytes[0x00..0x04].copy_from_slice(b"SFCI");
        req_bytes[0x08..0x0C].copy_from_slice(&1u32.to_le_bytes()); // Cmd 1: GetService

        // Payload: "time\0\0\0\0"
        req_bytes[0x10..0x18].copy_from_slice(b"time\0\0\0\0");

        let req = CmifRequest::parse(&req_bytes).unwrap();
        let mut ctx = IpcContext::new(req);

        sm.dispatch(&mut ctx).unwrap();
        assert_eq!(ctx.response.result_code, 0);
        assert_eq!(ctx.spawned_sessions.len(), 1);
        assert_eq!(ctx.spawned_sessions[0].0, "time");
    }
}
