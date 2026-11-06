use std::{
    fmt,
    io::{self, Write},
};

/// A trait for threadsafe writers used by tracing layers.
///
/// Writers implementing this trait are expected to handle errors internally.
/// For background/async writers, errors should be stored and reported during shutdown.
/// For synchronous writers (stdout), unrecoverable errors should panic.
///
/// This infallible design simplifies consumer code and reflects the reality that
/// telemetry write errors are typically non-recoverable at the call site.
pub trait SharedWriter: Send + Sync {
    /// Write `data` followed by a newline to the underlying writer.
    ///
    /// `data` is formatted straight into the writer's destination, so callers should
    /// pass what they would format rather than build a `String` first. `data` must
    /// only fail to format when the destination does.
    ///
    /// Implementations must handle errors internally. Background writers should
    /// store errors for later reporting during shutdown. Synchronous writers
    /// should panic on unrecoverable errors.
    fn writeln(&self, data: &dyn fmt::Display);

    fn is_terminal(&self) -> bool {
        false
    }
}

impl SharedWriter for io::Stdout {
    fn writeln(&self, data: &dyn fmt::Display) {
        // Lock stdout for the duration of the write operation
        let mut handle = self.lock();

        // Write the data, panic on error as this is unrecoverable
        writeln!(handle, "{data}").expect("failed to write to stdout");

        // Immediately flush to ensure data is written
        handle.flush().expect("failed to flush stdout");
    }

    fn is_terminal(&self) -> bool {
        io::IsTerminal::is_terminal(self)
    }
}
