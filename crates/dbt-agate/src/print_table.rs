use std::{fmt, io};

use crate::AgateTable;

pub struct TableDisplay<'a> {
    table: &'a AgateTable,
    pub max_rows: usize,
    pub max_columns: usize,
    pub max_column_width: usize,
}

impl<'a> TableDisplay<'a> {
    pub fn new(table: &'a AgateTable) -> Self {
        Self {
            table,
            max_rows: 20,
            max_columns: 6,
            max_column_width: 20,
        }
    }

    pub fn with_max_rows(mut self, max_rows: usize) -> Self {
        self.max_rows = max_rows;
        self
    }

    pub fn with_max_columns(mut self, max_columns: usize) -> Self {
        self.max_columns = max_columns;
        self
    }

    pub fn with_max_column_width(mut self, max_column_width: usize) -> Self {
        self.max_column_width = max_column_width;
        self
    }
}

impl fmt::Display for TableDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut writer = FormatterAsWriter::new(f);
        print_table(
            self.table,
            self.max_rows,
            self.max_columns,
            self.max_column_width,
            Some(&mut writer),
        )
        .map_err(move |_| fmt::Error)
    }
}

#[inline(never)]
pub fn print_table(
    table: &AgateTable,
    max_rows: usize,
    max_columns: usize,
    max_column_width: usize,
    output: Option<&mut dyn io::Write>,
) -> Result<(), io::Error> {
    // Parse arguments or use defaults matching Python implementation
    // Extract arguments if provided

    let mut stdout = io::stdout();
    let out = output.unwrap_or(&mut stdout);

    // Get character constants (equivalent to Python config options)
    let ellipsis = "...";
    let truncation = "...";
    let h_line = "-";
    let v_line = "|";

    // Get column names and truncate if needed
    let mut display_column_names = Vec::new();
    for name in table.column_names_iter().take(max_columns) {
        if name.len() > max_column_width {
            display_column_names.push(format!(
                "{}{}",
                &name[..max_column_width - truncation.len()],
                truncation
            ));
        } else {
            display_column_names.push(name.to_string());
        }
    }

    let columns_truncated = max_columns < table.num_columns();
    if columns_truncated {
        display_column_names.push(ellipsis.to_string());
    }

    // Calculate initial column widths based on headers
    let mut widths = display_column_names
        .iter()
        .map(|name| name.len())
        .collect::<Vec<usize>>();

    // Format the data
    let mut formatted_data = Vec::new();
    let num_rows = table.num_rows();
    let rows_truncated = max_rows < num_rows;

    for i in 0..std::cmp::min(max_rows, num_rows) {
        let mut formatted_row = Vec::new();

        for j in 0..std::cmp::min(max_columns, table.num_columns()) {
            if let Some(cell) = table.cell(i as isize, j as isize) {
                let value = if cell.is_undefined() || cell.is_none() {
                    "".to_string()
                } else {
                    let str_val = cell.to_string();
                    if str_val.len() > max_column_width {
                        format!(
                            "{}{}",
                            &str_val[..max_column_width - truncation.len()],
                            truncation
                        )
                    } else {
                        str_val
                    }
                };

                // Update column width if necessary
                if j < widths.len() && value.len() > widths[j] {
                    widths[j] = value.len();
                }

                formatted_row.push(value);
            } else {
                formatted_row.push("".to_string());
            }
        }

        if columns_truncated {
            formatted_row.push(ellipsis.to_string());
        }

        formatted_data.push(formatted_row);
    }

    // Helper function to write a row
    let write_row = |out: &mut dyn io::Write,
                     row: &[String],
                     is_header: bool|
     -> Result<(), io::Error> {
        out.write_all(v_line.as_bytes())?;
        for (j, value) in row.iter().enumerate() {
            if j < widths.len() {
                // Determine if it's a number or text for alignment
                // Here we're simplifying by just checking if it parses as a number
                let is_number =
                    value.parse::<f64>().is_ok() && !value.is_empty() && value != ellipsis;

                if is_number || is_header {
                    // Right justify numbers and headers
                    out.write_fmt(format_args!(" {} ", value.to_string().pad_left(widths[j])))?;
                } else {
                    // Left justify text
                    out.write_fmt(format_args!(" {} ", value.to_string().pad_right(widths[j])))?;
                }
            }

            if j < row.len() - 1 {
                out.write_all(v_line.as_bytes())?;
            }
        }
        out.write_fmt(format_args!("{}\n", v_line))?;
        Ok(())
    };

    // Write header row
    write_row(out, &display_column_names, true)?;

    // Write divider
    out.write_all(v_line.as_bytes())?;
    for (j, &width) in widths.iter().enumerate() {
        out.write_fmt(format_args!(" {} ", h_line.repeat(width)))?;

        if j < widths.len() - 1 {
            out.write_all(v_line.as_bytes())?;
        }
    }
    out.write_fmt(format_args!("{}\n", v_line))?;

    // Write data rows
    for row in formatted_data {
        write_row(out, &row, false)?;
    }

    // Add truncation row if rows were truncated
    if rows_truncated {
        let ellipsis_row = vec![ellipsis.to_string(); display_column_names.len()];
        write_row(out, &ellipsis_row, false)?;
    }

    Ok(())
}

// Add helper methods for string padding - used in print_table
trait StringPadding {
    fn pad_right(&self, width: usize) -> String;
    fn pad_left(&self, width: usize) -> String;
}

impl StringPadding for String {
    fn pad_right(&self, width: usize) -> String {
        if self.len() >= width {
            self.clone()
        } else {
            let mut padded = self.clone();
            padded.push_str(&" ".repeat(width - self.len()));
            padded
        }
    }

    fn pad_left(&self, width: usize) -> String {
        if self.len() >= width {
            self.clone()
        } else {
            let mut padded = " ".repeat(width - self.len());
            padded.push_str(self);
            padded
        }
    }
}

impl StringPadding for str {
    fn pad_right(&self, width: usize) -> String {
        self.to_string().pad_right(width)
    }

    fn pad_left(&self, width: usize) -> String {
        self.to_string().pad_left(width)
    }
}

/// Adapter to use a `fmt::Formatter` as an `io::Write`.
///
/// This is only necessary because [fmt::Formatter::new] is not available
/// on stable Rust yet [1].
///
/// [1] https://github.com/rust-lang/rust/issues/118117
pub struct FormatterAsWriter<'a, 'b> {
    f: &'a mut fmt::Formatter<'b>,
}

impl<'a, 'b> FormatterAsWriter<'a, 'b> {
    pub fn new(f: &'a mut fmt::Formatter<'b>) -> Self {
        Self { f }
    }
}

impl<'a, 'b> io::Write for FormatterAsWriter<'a, 'b> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // XXX: this awkward conversion won't be necessary once
        // [1] is stabilized.
        let s = std::str::from_utf8(buf).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })?;
        self.f.write_str(s).map_err(io::Error::other)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
