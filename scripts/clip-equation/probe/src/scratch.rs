//! A temporary folder for raw-frame writes, on the volume that holds
//! %LOCALAPPDATA%. It owns the owner's disk limits for this probe: at most
//! `MAX_WRITE_BYTES` written per run, free space never below `MIN_FREE_BYTES`,
//! and everything deleted afterwards (also on an error exit, through Drop).

use std::fs;
use std::path::PathBuf;

use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::core::HSTRING;

use crate::{Result, win};

/// The owner's cap for one run of a probe (3 GB).
pub const MAX_WRITE_BYTES: u64 = 3_000_000_000;
/// Free space that must remain on the volume (4 GiB, so never below 4 GB).
pub const MIN_FREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub struct ScratchFolder {
    path: PathBuf,
    written: u64,
    files: u64,
}

impl ScratchFolder {
    pub fn create(tag: &str) -> Result<Self> {
        let base =
            std::env::var_os("LOCALAPPDATA").ok_or_else(|| "LOCALAPPDATA is not set".to_owned())?;
        let path = PathBuf::from(base)
            .join("Temp")
            .join(format!("clip-probe-{tag}-{}", std::process::id()));
        fs::create_dir_all(&path).map_err(|error| format!("create {}: {error}", path.display()))?;
        Ok(Self {
            path,
            written: 0,
            files: 0,
        })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub const fn written(&self) -> u64 {
        self.written
    }

    pub fn free_bytes(&self) -> Result<u64> {
        let mut free = 0;
        let path = HSTRING::from(self.path.as_os_str());
        // SAFETY: one out-parameter.
        unsafe { GetDiskFreeSpaceExW(&path, Some(&raw mut free), None, None) }
            .map_err(win("GetDiskFreeSpaceExW"))?;
        Ok(free)
    }

    /// Refuses a phase that would pass the write cap or the free-space floor.
    pub fn reserve(&self, bytes: u64) -> Result<()> {
        if self.written + bytes > MAX_WRITE_BYTES {
            return Err(format!(
                "refused: {} + {bytes} bytes passes the {MAX_WRITE_BYTES}-byte cap",
                self.written
            ));
        }
        let free = self.free_bytes()?;
        if free < bytes + MIN_FREE_BYTES {
            return Err(format!(
                "refused: {free} bytes free, {bytes} to write, {MIN_FREE_BYTES} must stay free"
            ));
        }
        Ok(())
    }

    /// The path of the next frame file; counts its bytes against the cap.
    pub fn next_file(&mut self, bytes: u64) -> Result<PathBuf> {
        if self.written + bytes > MAX_WRITE_BYTES {
            return Err("refused: write cap reached".to_owned());
        }
        self.written += bytes;
        self.files += 1;
        Ok(self.path.join(format!("frame-{:06}.raw", self.files)))
    }

    /// Writes one raw frame as one file, through the file cache.
    pub fn write_frame(&mut self, data: &[u8]) -> Result<()> {
        let path = self.next_file(data.len() as u64)?;
        fs::write(&path, data).map_err(|error| format!("write {}: {error}", path.display()))
    }

    /// Deletes every file written so far; the folder stays.
    pub fn clear(&self) -> Result<()> {
        for entry in fs::read_dir(&self.path).map_err(|error| format!("read_dir: {error}"))? {
            let entry = entry.map_err(|error| format!("read_dir entry: {error}"))?;
            fs::remove_file(entry.path())
                .map_err(|error| format!("remove {}: {error}", entry.path().display()))?;
        }
        Ok(())
    }
}

impl Drop for ScratchFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
