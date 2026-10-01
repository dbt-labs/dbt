use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Error};
use tempfile::NamedTempFile;

pub const STDIN_STDOUT: &str = "-";

pub struct Output {
    temp: Option<(PathBuf, NamedTempFile)>,
}

impl Output {
    pub fn new(filename: &Path) -> Result<Output, Error> {
        Ok(Output {
            temp: if filename == Path::new(STDIN_STDOUT) {
                None
            } else {
                let filename = std::env::current_dir()?.join(filename);
                let ntf = NamedTempFile::new_in(
                    filename
                        .parent()
                        .ok_or_else(|| anyhow!("cannot write to root"))?,
                )?;
                Some((filename.to_path_buf(), ntf))
            },
        })
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        if let Some((filename, temp)) = self.temp.take() {
            temp.persist(filename)?;
        }
        Ok(())
    }
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.temp {
            Some((_, ref mut out)) => out.write(buf),
            None => std::io::stdout().write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.temp {
            Some((_, ref mut out)) => out.flush(),
            None => std::io::stdout().flush(),
        }
    }
}
