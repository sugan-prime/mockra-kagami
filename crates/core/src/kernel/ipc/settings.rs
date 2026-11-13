//! Horizon OS System Settings Services (`set:sys`, `set`) implementation.
//!
//! Provides system language, country/region, and UI color theme configuration.

use super::{IpcContext, IpcError, ServiceHandler};

pub struct SettingsService;

impl Default for SettingsService {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsService {
    pub fn new() -> Self {
        Self
    }
}

impl ServiceHandler for SettingsService {
    fn service_name(&self) -> &str {
        "set:sys"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: GetLanguageCode() -> language_code: u64
            0 => {
                // "en-US" formatted as an 8-byte ASCII string
                let mut lang = [0u8; 8];
                lang[..5].copy_from_slice(b"en-US");
                let code = u64::from_le_bytes(lang);

                context.response.push_u64(code);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: GetAvailableLanguageCodes() -> count: u32, codes: [u64]
            1 => {
                let mut lang_en = [0u8; 8];
                lang_en[..5].copy_from_slice(b"en-US");
                let mut lang_ja = [0u8; 8];
                lang_ja[..5].copy_from_slice(b"ja-JP");

                context.response.push_u32(2); // 2 languages
                context.response.push_u64(u64::from_le_bytes(lang_en));
                context.response.push_u64(u64::from_le_bytes(lang_ja));
                context.response.result_code = 0;
                Ok(())
            }

            // Command 3: GetColorSetId() -> color_set: u32 (0 = Light, 1 = Dark)
            3 => {
                context.response.push_u32(1); // Basic Black / Dark theme
                context.response.result_code = 0;
                Ok(())
            }

            // Command 4: GetRegionCode() -> region: u32 (0 = JP, 1 = US, 2 = EU)
            4 => {
                context.response.push_u32(1); // USA
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
    fn test_settings_service_get_language_and_region() {
        let set = SettingsService::new();

        // GetLanguageCode
        let req_lang = CmifRequest::new(0, vec![]);
        let mut ctx_lang = IpcContext::new(req_lang);
        set.dispatch(&mut ctx_lang).unwrap();

        assert_eq!(ctx_lang.response.result_code, 0);
        let lang_u64 = u64::from_le_bytes(ctx_lang.response.raw_data[..8].try_into().unwrap());
        let lang_bytes = lang_u64.to_le_bytes();
        let lang_str = String::from_utf8_lossy(&lang_bytes);
        assert!(lang_str.starts_with("en-US"));

        // GetRegionCode
        let req_reg = CmifRequest::new(4, vec![]);
        let mut ctx_reg = IpcContext::new(req_reg);
        set.dispatch(&mut ctx_reg).unwrap();

        assert_eq!(ctx_reg.response.result_code, 0);
        let region = u32::from_le_bytes(ctx_reg.response.raw_data[..4].try_into().unwrap());
        assert_eq!(region, 1); // US
    }
}
