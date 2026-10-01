//! Dump the contents of a record/replay `recordings.db` sqlite file.
//!
//! Usage: `dump-recordings <path/to/recordings.db>`

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: dump-recordings <path/to/recordings.db>");
        return ExitCode::from(2);
    };

    match adbc_record_replay::debug_recording_file(std::io::stdout(), &PathBuf::from(&path)) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::from(1)
        }
    }
}
