pub mod account;
pub mod applet;
pub mod cmif;
pub mod hid;
pub mod settings;
pub mod sm;
pub mod time;

pub use account::AccountService;
pub use applet::AppletManagerService;
pub use cmif::{CmifError, CmifRequest, CmifResponse};
pub use hid::{buttons, style_set, AnalogStick, ControllerState, HidService, SixAxisMotion};
pub use settings::SettingsService;
pub use sm::ServiceManager;
pub use time::TimeService;

use crate::kernel::object::KEvent;
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum IpcError {
    #[error("Unknown or unimplemented IPC Command ID: {0}")]
    UnknownCommand(u32),
    #[error("Service not found: {0}")]
    ServiceNotFound(String),
    #[error("CMIF error: {0}")]
    Cmif(#[from] CmifError),
    #[error("Invalid request payload: {0}")]
    InvalidPayload(&'static str),
}

/// Context passed to a service dispatch call.
pub struct IpcContext {
    pub request: CmifRequest,
    pub response: CmifResponse,
    /// New service sessions generated during dispatch (e.g. from `sm:GetService`).
    pub spawned_sessions: Vec<(String, Arc<dyn ServiceHandler>)>,
    /// Readable event handles created and returned to caller (e.g. `RegisterBufferEvent`).
    pub spawned_events: Vec<KEvent>,
}

impl IpcContext {
    pub fn new(request: CmifRequest) -> Self {
        Self {
            request,
            response: CmifResponse::success(),
            spawned_sessions: Vec::new(),
            spawned_events: Vec::new(),
        }
    }
}

/// Interface implemented by all High-Level Emulation (HLE) Horizon OS services.
pub trait ServiceHandler: Send + Sync {
    /// Canonical service name (e.g. "sm:", "time:", "hid:").
    fn service_name(&self) -> &str;

    /// Dispatches a command based on its CMIF Command ID.
    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError>;
}
