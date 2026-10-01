use std::{cell::RefCell, fmt, rc::Rc};

/// Tracks the current location (line, column, byte index) in the output buffer.
/// This is used for macro span tracking during rendering.
#[derive(Debug)]
pub struct OutputTrackerLocation {
    line: RefCell<u32>,
    col: RefCell<u32>,
    index: RefCell<u32>,
}

impl Default for OutputTrackerLocation {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputTrackerLocation {
    /// Creates a new `OutputTrackerLocation` with line=1, col=1, index=0.
    pub fn new() -> Self {
        Self {
            line: RefCell::new(1),
            col: RefCell::new(1),
            index: RefCell::new(0),
        }
    }

    /// Returns the current line number (1-based).
    pub fn line(&self) -> u32 {
        *self.line.borrow()
    }

    /// Returns the current column number.
    pub fn col(&self) -> u32 {
        *self.col.borrow()
    }

    /// Returns the current byte index in the output.
    pub fn index(&self) -> u32 {
        *self.index.borrow()
    }
}

/// A `fmt::Write` implementation that tracks the current location in the output.
pub struct OutputTracker<'a> {
    w: &'a mut (dyn fmt::Write + 'a),
    /// The current location in the output.
    pub location: Rc<OutputTrackerLocation>,
}

impl<'a> OutputTracker<'a> {
    /// Creates a new `OutputTracker` that writes to the given writer.
    pub fn new(w: &'a mut (dyn fmt::Write + 'a)) -> Self {
        OutputTracker {
            w,
            location: Rc::new(OutputTrackerLocation {
                line: RefCell::new(1),
                col: RefCell::new(1),
                index: RefCell::new(0),
            }),
        }
    }

    /// Creates a new `OutputTracker` that writes to the given writer,
    /// using an existing location tracker.
    pub fn with_location(
        w: &'a mut (dyn fmt::Write + 'a),
        location: Rc<OutputTrackerLocation>,
    ) -> Self {
        OutputTracker { w, location }
    }
}

impl fmt::Write for OutputTracker<'_> {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        *self.location.line.borrow_mut() += s.chars().filter(|&c| c == '\n').count() as u32;
        *self.location.col.borrow_mut() = if let Some(last) = s.rfind('\n') {
            (s.len() - last) as u32
        } else {
            *self.location.col.borrow() + s.len() as u32
        };
        *self.location.index.borrow_mut() += s.len() as u32;

        self.w.write_str(s)
    }
}
