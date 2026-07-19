use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::Path,
};

use fs2::FileExt;

use crate::error::{CoreError, CoreResult};

pub enum InstanceLock {
    Acquired(InstanceGuard),
    AlreadyRunning,
}

pub struct InstanceGuard {
    file: File,
}

impl InstanceGuard {
    pub fn acquire(data_dir: &Path) -> CoreResult<InstanceLock> {
        fs::create_dir_all(data_dir).map_err(|error| {
            CoreError::Config(format!("could not create {}: {error}", data_dir.display()))
        })?;
        let path = data_dir.join("core.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                CoreError::Config(format!("could not open {}: {error}", path.display()))
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(InstanceLock::Acquired(Self { file })),
            Err(error) if lock_is_held(&error) => Ok(InstanceLock::AlreadyRunning),
            Err(error) => Err(CoreError::Config(format!(
                "could not lock {}: {error}",
                path.display()
            ))),
        }
    }
}

fn lock_is_held(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::WouldBlock
        // Windows reports ERROR_LOCK_VIOLATION for LockFileEx contention.
        || cfg!(windows) && error.raw_os_error() == Some(33)
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn data_directory_lock_is_singleton_and_recoverable() {
        let dir = TempDir::new().unwrap();
        let first = InstanceGuard::acquire(dir.path()).unwrap();
        assert!(matches!(first, InstanceLock::Acquired(_)));
        assert!(matches!(
            InstanceGuard::acquire(dir.path()).unwrap(),
            InstanceLock::AlreadyRunning
        ));
        drop(first);
        assert!(matches!(
            InstanceGuard::acquire(dir.path()).unwrap(),
            InstanceLock::Acquired(_)
        ));
    }
}
