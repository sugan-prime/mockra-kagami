//! Horizon OS Applet Manager Services (`appletOE`, `appletAE`) implementation.
//!
//! Provides application lifecycle state, focus management, operation mode (Docked/Handheld),
//! and display layer management.

use super::{IpcContext, IpcError, ServiceHandler};
use crate::kernel::object::KEvent;
use std::sync::Arc;

pub struct ICommonStateGetter {
    pub state_event: KEvent,
}

impl Default for ICommonStateGetter {
    fn default() -> Self {
        Self::new()
    }
}

impl ICommonStateGetter {
    pub fn new() -> Self {
        Self {
            state_event: KEvent::new(true),
        }
    }
}

impl ServiceHandler for ICommonStateGetter {
    fn service_name(&self) -> &str {
        "ICommonStateGetter"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetEventHandle() -> event_handle
            0 => {
                context.spawned_events.push(self.state_event.clone());
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: ReceiveMessage() -> message: u32 (0x0F = FocusStateChanged)
            1 => {
                context.response.push_u32(0x0F);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 5: GetOperationMode() -> mode: u8 (0 = Handheld, 1 = Docked)
            5 => {
                context.response.push_bytes(&[1]); // Docked mode
                context.response.result_code = 0;
                Ok(())
            }

            // Command 9: GetCurrentFocusState() -> state: u8 (1 = InFocus)
            9 => {
                context.response.push_bytes(&[1]); // In Focus
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

pub struct ISelfController;

impl ServiceHandler for ISelfController {
    fn service_name(&self) -> &str {
        "ISelfController"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: Exit()
            0 | 1 | 2 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 40: CreateManagedDisplayLayer() -> layer_id: u64
            40 => {
                context.response.push_u64(1); // Layer ID 1
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

pub struct IWindowController;

impl ServiceHandler for IWindowController {
    fn service_name(&self) -> &str {
        "IWindowController"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 1: GetAppletResourceUserId() -> user_id: u64
            1 => {
                context.response.push_u64(1);
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

pub struct IApplicationProxy {
    common_state: Arc<ICommonStateGetter>,
    self_controller: Arc<ISelfController>,
    window_controller: Arc<IWindowController>,
}

impl Default for IApplicationProxy {
    fn default() -> Self {
        Self::new()
    }
}

impl IApplicationProxy {
    pub fn new() -> Self {
        Self {
            common_state: Arc::new(ICommonStateGetter::new()),
            self_controller: Arc::new(ISelfController),
            window_controller: Arc::new(IWindowController),
        }
    }
}

impl ServiceHandler for IApplicationProxy {
    fn service_name(&self) -> &str {
        "IApplicationProxy"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetCommonStateGetter() -> session_handle
            0 => {
                context.spawned_sessions.push(("ICommonStateGetter".into(), self.common_state.clone()));
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: GetSelfController() -> session_handle
            1 => {
                context.spawned_sessions.push(("ISelfController".into(), self.self_controller.clone()));
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: GetWindowController() -> session_handle
            2 => {
                context.spawned_sessions.push(("IWindowController".into(), self.window_controller.clone()));
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

pub struct IAllSystemAppletProxiesService {
    app_proxy: Arc<IApplicationProxy>,
}

impl Default for IAllSystemAppletProxiesService {
    fn default() -> Self {
        Self::new()
    }
}

impl IAllSystemAppletProxiesService {
    pub fn new() -> Self {
        Self {
            app_proxy: Arc::new(IApplicationProxy::new()),
        }
    }
}

impl ServiceHandler for IAllSystemAppletProxiesService {
    fn service_name(&self) -> &str {
        "IAllSystemAppletProxiesService"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 100: OpenApplicationProxy() -> session_handle
            100 => {
                context.spawned_sessions.push(("IApplicationProxy".into(), self.app_proxy.clone()));
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

/// Service handler for `appletOE` and `appletAE`.
pub struct AppletManagerService {
    proxies_service: Arc<IAllSystemAppletProxiesService>,
}

impl Default for AppletManagerService {
    fn default() -> Self {
        Self::new()
    }
}

impl AppletManagerService {
    pub fn new() -> Self {
        Self {
            proxies_service: Arc::new(IAllSystemAppletProxiesService::new()),
        }
    }
}

impl ServiceHandler for AppletManagerService {
    fn service_name(&self) -> &str {
        "appletOE"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: OpenSession() -> session_handle
            0 => {
                context.spawned_sessions.push(("IAllSystemAppletProxiesService".into(), self.proxies_service.clone()));
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
    fn test_applet_proxy_and_operation_mode() {
        let applet_service = AppletManagerService::new();

        // 1. OpenSession
        let req_open = CmifRequest::new(0, vec![]);
        let mut ctx_open = IpcContext::new(req_open);
        applet_service.dispatch(&mut ctx_open).unwrap();

        assert_eq!(ctx_open.response.result_code, 0);
        let proxies_session = ctx_open.spawned_sessions[0].1.clone();

        // 2. OpenApplicationProxy
        let req_app = CmifRequest::new(100, vec![]);
        let mut ctx_app = IpcContext::new(req_app);
        proxies_session.dispatch(&mut ctx_app).unwrap();

        let app_proxy_session = ctx_app.spawned_sessions[0].1.clone();

        // 3. GetCommonStateGetter
        let req_common = CmifRequest::new(0, vec![]);
        let mut ctx_common = IpcContext::new(req_common);
        app_proxy_session.dispatch(&mut ctx_common).unwrap();

        let common_state_session = ctx_common.spawned_sessions[0].1.clone();

        // 4. Query operation mode (Cmd 5: Docked)
        let req_mode = CmifRequest::new(5, vec![]);
        let mut ctx_mode = IpcContext::new(req_mode);
        common_state_session.dispatch(&mut ctx_mode).unwrap();

        assert_eq!(ctx_mode.response.raw_data[0], 1); // Docked
    }
}
