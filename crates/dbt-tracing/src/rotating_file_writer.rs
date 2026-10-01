use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Synchronous size-based rotating file writer.
///
/// This writer is intended to be wrapped by `BackgroundWriter` so rotation and
/// writes are performed on a dedicated worker thread.
pub struct RotatingFileWriter {
    base_path: PathBuf,
    file: Option<File>,
    current_size: u64,
    max_bytes: u64,
    backup_count: usize,
}

impl RotatingFileWriter {
    pub fn new<P: AsRef<Path>>(path: P, max_bytes: u64, backup_count: usize) -> io::Result<Self> {
        let base_path = path.as_ref().to_path_buf();
        let file = Self::open_append(&base_path)?;
        let current_size = file.metadata()?.len();

        Ok(Self {
            base_path,
            file: Some(file),
            current_size,
            max_bytes,
            backup_count,
        })
    }

    fn open_append(path: &Path) -> io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    fn log_file_path(path: &Path, backup_index: usize) -> PathBuf {
        if backup_index == 0 {
            return path.to_path_buf();
        }

        let mut os: OsString = path.as_os_str().to_owned();
        os.push(format!(".{backup_index}"));
        PathBuf::from(os)
    }

    fn maybe_rotate_for_write(&mut self, incoming_bytes: usize) -> io::Result<()> {
        if self.max_bytes != 0
            && self.current_size.saturating_add(incoming_bytes as u64) > self.max_bytes
        {
            self.rotate()?;
        }

        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let mut file = self.file.take().expect("file should be present");
        file.flush()?;
        drop(file);

        for idx in (0..self.backup_count).rev() {
            let src = Self::log_file_path(&self.base_path, idx);
            let dst = Self::log_file_path(&self.base_path, idx + 1);

            if src.exists() {
                std::fs::rename(src, dst)?;
            }
        }

        let file = Self::open_append(&self.base_path)?;
        self.file = Some(file);
        self.current_size = 0;
        Ok(())
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.maybe_rotate_for_write(buf.len())?;

        let written = {
            let file = self.file.as_mut().expect("file should be present");
            file.write(buf)?
        };
        self.current_size = self.current_size.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let file = self.file.as_mut().expect("file should be present");
        file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::RotatingFileWriter;
    use std::io::{self, Write};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_root(test_name: &str) -> (TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let root = temp_dir.path().join("root").join(test_name);
        std::fs::create_dir_all(&root).expect("create test root");
        (temp_dir, root)
    }

    #[test]
    fn rotates_when_threshold_exceeded() {
        let (_temp_dir, root) = create_test_root("rotates_when_threshold_exceeded");
        let base = root.join("dbt.log");

        let mut writer = RotatingFileWriter::new(&base, 8, 2).expect("create writer");
        writer.write_all(b"1234").expect("first write");
        writer.write_all(b"5678").expect("second write");
        writer.write_all(b"9").expect("rotate write");
        writer.flush().expect("flush");

        let current = std::fs::read_to_string(&base).expect("read current");
        let backup_1 = std::fs::read_to_string(format!("{}.1", base.display())).expect("read .1");

        assert_eq!(current, "9");
        assert_eq!(backup_1, "12345678");
    }

    #[test]
    fn respects_existing_file_size() {
        let (_temp_dir, root) = create_test_root("respects_existing_file_size");
        let base = root.join("dbt.log");
        std::fs::write(&base, b"1234567").expect("seed file");

        let mut writer = RotatingFileWriter::new(&base, 8, 5).expect("create writer");
        writer.write_all(b"8").expect("append within limit");
        writer.write_all(b"9").expect("rotate");
        writer.flush().expect("flush");

        let current = std::fs::read_to_string(&base).expect("read current");
        let backup_1 = std::fs::read_to_string(format!("{}.1", base.display())).expect("read .1");

        assert_eq!(current, "9");
        assert_eq!(backup_1, "12345678");
    }

    #[test]
    fn zero_limit_disables_rotation() {
        let (_temp_dir, root) = create_test_root("zero_limit_disables_rotation");
        let base = root.join("dbt.log");

        let mut writer = RotatingFileWriter::new(&base, 0, 5).expect("create writer");
        writer.write_all(b"123456789").expect("write");
        writer.flush().expect("flush");

        let current = std::fs::read_to_string(&base).expect("read current");
        assert_eq!(current, "123456789");
        assert!(!std::path::Path::new(&format!("{}.1", base.display())).exists());
    }

    #[test]
    fn zero_backup_count_rotates_into_base_log_file() {
        let (_temp_dir, root) = create_test_root("zero_backup_count_rotates_into_base_log_file");
        let base = root.join("dbt.log");
        std::fs::write(&base, b"1234").expect("seed file");

        assert_eq!(RotatingFileWriter::log_file_path(&base, 0), base);

        let mut writer = RotatingFileWriter::new(&base, 4, 0).expect("create writer");
        writer.write_all(b"5").expect("rotate write");
        writer.flush().expect("flush");

        let current = std::fs::read_to_string(&base).expect("read current");

        assert_eq!(current, "12345");
        assert!(!std::path::Path::new(&format!("{}.1", base.display())).exists());
    }

    #[test]
    fn rotation_overwrites_existing_oldest_backup() {
        let (_temp_dir, root) = create_test_root("rotation_overwrites_existing_oldest_backup");
        let base = root.join("dbt.log");

        std::fs::write(&base, b"1234").expect("seed file");
        std::fs::write(root.join("dbt.log.1"), b"older-backup").expect("seed .1");
        std::fs::write(root.join("dbt.log.2"), b"oldest-backup").expect("seed .2");

        let mut writer = RotatingFileWriter::new(&base, 4, 2).expect("create writer");
        writer.write_all(b"5").expect("rotate write");
        writer.flush().expect("flush");

        let current = std::fs::read_to_string(&base).expect("read current");
        let backup_1 = std::fs::read_to_string(root.join("dbt.log.1")).expect("read .1");
        let backup_2 = std::fs::read_to_string(root.join("dbt.log.2")).expect("read .2");

        assert_eq!(current, "5");
        assert_eq!(backup_1, "1234");
        assert_eq!(backup_2, "older-backup");
    }

    #[test]
    fn rotation_handles_skipped_existing_backups() {
        let (_temp_dir, root) = create_test_root("rotation_handles_skipped_existing_backups");
        let base = root.join("dbt.log");

        std::fs::write(&base, b"1234").expect("seed file");
        std::fs::write(root.join("dbt.log.1"), b"backup-one").expect("seed .1");
        std::fs::write(root.join("dbt.log.3"), b"backup-three").expect("seed .3");

        let mut writer = RotatingFileWriter::new(&base, 4, 3).expect("create writer");
        writer.write_all(b"5").expect("rotate write");
        writer.flush().expect("flush");

        let current = std::fs::read_to_string(&base).expect("read current");
        let backup_1 = std::fs::read_to_string(root.join("dbt.log.1")).expect("read .1");
        let backup_2 = std::fs::read_to_string(root.join("dbt.log.2")).expect("read .2");
        let backup_3 = std::fs::read_to_string(root.join("dbt.log.3")).expect("read .3");

        assert_eq!(current, "5");
        assert_eq!(backup_1, "1234");
        assert_eq!(backup_2, "backup-one");
        assert_eq!(backup_3, "backup-three");
    }

    #[test]
    fn fails_if_destination_backup_cannot_be_overwritten() {
        let (_temp_dir, root) =
            create_test_root("fails_if_destination_backup_cannot_be_overwritten");
        let base = root.join("dbt.log");
        let conflicting_destination = root.join("dbt.log.2");

        std::fs::write(root.join("dbt.log.1"), b"backup-one").expect("seed .1");
        std::fs::create_dir_all(&conflicting_destination).expect("create conflicting destination");

        let mut writer = RotatingFileWriter::new(&base, 4, 2).expect("create writer");
        writer.write_all(b"1234").expect("seed file");

        let error = writer
            .write_all(b"5")
            .expect_err("rotation should fail if destination backup cannot be removed");
        assert!(error.kind() != io::ErrorKind::WouldBlock);
    }
}
