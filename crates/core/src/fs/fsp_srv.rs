//! Filesystem Proxy Service (`fsp-srv`) implementation.
//!
//! Provides the primary Horizon OS filesystem IPC interface for mounting RomFS,
//! opening save data filesystems, and reading/writing game files.

use crate::fs::{FileMode, IFile, IFileSystem};
use crate::kernel::ipc::{IpcContext, IpcError, ServiceHandler};
use std::sync::{Arc, Mutex};

/// Service handler for an open `IFile` session.
pub struct IFileService {
    file: Mutex<Box<dyn IFile>>,
}

impl IFileService {
    pub fn new(file: Box<dyn IFile>) -> Self {
        Self {
            file: Mutex::new(file),
        }
    }
}

impl ServiceHandler for IFileService {
    fn service_name(&self) -> &str {
        "IFile"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: Read(offset: u64, size: u64) -> bytes
            0 => {
                let offset = context.request.read_u64(0).unwrap_or(0);
                let size = context.request.read_u64(8).unwrap_or(0) as usize;

                let mut buf = vec![0u8; size.min(0x1000)];
                let file = self.file.lock().unwrap();
                let bytes_read = file.read(offset, &mut buf).map_err(|_| IpcError::InvalidPayload("Read error"))?;

                context.response.push_u64(bytes_read as u64);
                context.response.push_bytes(&buf[..bytes_read]);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 1: Write(offset: u64)
            1 => {
                let offset = context.request.read_u64(0).unwrap_or(0);
                let data = if context.request.raw_data.len() > 8 {
                    &context.request.raw_data[8..]
                } else {
                    &[]
                };

                let mut file = self.file.lock().unwrap();
                let written = file.write(offset, data).map_err(|_| IpcError::InvalidPayload("Write error"))?;
                context.response.push_u64(written as u64);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 4: GetSize() -> size: u64
            4 => {
                let file = self.file.lock().unwrap();
                let size = file.get_size().unwrap_or(0);
                context.response.push_u64(size);
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

/// Service handler for an open `IFileSystem` session.
pub struct IFileSystemService {
    fs: Arc<dyn IFileSystem>,
}

impl IFileSystemService {
    pub fn new(fs: Arc<dyn IFileSystem>) -> Self {
        Self { fs }
    }
}

impl ServiceHandler for IFileSystemService {
    fn service_name(&self) -> &str {
        "IFileSystem"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: OpenFile(mode: u32, path: string) -> IFile session handle
            0 => {
                let mode_val = context.request.read_u32(0).unwrap_or(1);
                let mode = match mode_val {
                    2 => FileMode::Write,
                    3 => FileMode::ReadWrite,
                    _ => FileMode::Read,
                };

                let path = context
                    .request
                    .read_cstring(4)
                    .unwrap_or_else(|| "/main".to_string());

                match self.fs.open_file(&path, mode) {
                    Ok(file) => {
                        let file_session = Arc::new(IFileService::new(file));
                        context
                            .spawned_sessions
                            .push(("IFile".to_string(), file_session));
                        context.response.result_code = 0;
                        Ok(())
                    }
                    Err(_) => {
                        context.response.result_code = 0x202; // Path not found
                        Ok(())
                    }
                }
            }

            // Command 7: GetEntryType(path: string) -> entry_type: u32 (1 = Directory, 2 = File)
            7 => {
                let path = context
                    .request
                    .read_cstring(0)
                    .unwrap_or_else(|| "/".to_string());

                match self.fs.get_entry_type(&path) {
                    Ok(entry_type) => {
                        let type_id = match entry_type {
                            crate::fs::EntryType::Directory => 1u32,
                            crate::fs::EntryType::File => 2u32,
                        };
                        context.response.push_u32(type_id);
                        context.response.result_code = 0;
                        Ok(())
                    }
                    Err(_) => {
                        context.response.result_code = 0x202; // Not found
                        Ok(())
                    }
                }
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

/// The root Filesystem Proxy Service (`fsp-srv`).
pub struct FileSystemProxyService {
    romfs: Option<Arc<dyn IFileSystem>>,
    save_fs: Option<Arc<dyn IFileSystem>>,
}

impl Default for FileSystemProxyService {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystemProxyService {
    pub fn new() -> Self {
        Self {
            romfs: None,
            save_fs: None,
        }
    }

    pub fn with_romfs(mut self, romfs: Arc<dyn IFileSystem>) -> Self {
        self.romfs = Some(romfs);
        self
    }

    pub fn with_save_data(mut self, save_fs: Arc<dyn IFileSystem>) -> Self {
        self.save_fs = Some(save_fs);
        self
    }
}

impl ServiceHandler for FileSystemProxyService {
    fn service_name(&self) -> &str {
        "fsp-srv"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: MountRomfs -> IFileSystem session handle
            0 => {
                if let Some(romfs) = &self.romfs {
                    let fs_session = Arc::new(IFileSystemService::new(romfs.clone()));
                    context
                        .spawned_sessions
                        .push(("IFileSystem".to_string(), fs_session));
                    context.response.result_code = 0;
                    Ok(())
                } else {
                    context.response.result_code = 0x202; // Not mounted
                    Ok(())
                }
            }

            // Command 1: Initialize
            1 => {
                context.response.result_code = 0;
                Ok(())
            }

            // Command 51: OpenSaveDataFileSystem -> IFileSystem session handle
            51 => {
                if let Some(save_fs) = &self.save_fs {
                    let fs_session = Arc::new(IFileSystemService::new(save_fs.clone()));
                    context
                        .spawned_sessions
                        .push(("IFileSystem".to_string(), fs_session));
                    context.response.result_code = 0;
                    Ok(())
                } else {
                    context.response.result_code = 0x202;
                    Ok(())
                }
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::romfs::RomfsBuilder;
    use crate::kernel::ipc::cmif::CmifRequest;

    #[test]
    fn test_fsp_srv_mount_romfs_and_read_file() {
        let mut builder = RomfsBuilder::new();
        builder.add_file("/title.txt", b"Mockra Kagami VFS".to_vec());
        let romfs = Arc::new(builder.build());

        let fsp = FileSystemProxyService::new().with_romfs(romfs);

        // 1. MountRomfs (Cmd 0)
        let mut req_buf = vec![0u8; 32];
        req_buf[0..4].copy_from_slice(b"SFCI");
        req_buf[8..12].copy_from_slice(&0u32.to_le_bytes()); // Cmd 0

        let req = CmifRequest::parse(&req_buf).unwrap();
        let mut ctx = IpcContext::new(req);
        fsp.dispatch(&mut ctx).unwrap();

        assert_eq!(ctx.response.result_code, 0);
        assert_eq!(ctx.spawned_sessions.len(), 1);
        let fs_session = ctx.spawned_sessions[0].1.clone();

        // 2. OpenFile("/title.txt") on IFileSystem session
        let mut open_buf = vec![0u8; 32];
        open_buf[0..4].copy_from_slice(b"SFCI");
        open_buf[8..12].copy_from_slice(&0u32.to_le_bytes()); // Cmd 0: OpenFile
        open_buf[16..20].copy_from_slice(&1u32.to_le_bytes()); // mode = Read
        open_buf[20..30].copy_from_slice(b"/title.txt");

        let open_req = CmifRequest::parse(&open_buf).unwrap();
        let mut open_ctx = IpcContext::new(open_req);
        fs_session.dispatch(&mut open_ctx).unwrap();

        assert_eq!(open_ctx.response.result_code, 0);
        assert_eq!(open_ctx.spawned_sessions.len(), 1);
        let file_session = open_ctx.spawned_sessions[0].1.clone();

        // 3. GetSize on IFile session
        let mut size_buf = vec![0u8; 32];
        size_buf[0..4].copy_from_slice(b"SFCI");
        size_buf[8..12].copy_from_slice(&4u32.to_le_bytes()); // Cmd 4: GetSize

        let size_req = CmifRequest::parse(&size_buf).unwrap();
        let mut size_ctx = IpcContext::new(size_req);
        file_session.dispatch(&mut size_ctx).unwrap();

        assert_eq!(size_ctx.response.result_code, 0);
        let file_size = u64::from_le_bytes(size_ctx.response.raw_data[0..8].try_into().unwrap());
        assert_eq!(file_size, 17);
    }
}
