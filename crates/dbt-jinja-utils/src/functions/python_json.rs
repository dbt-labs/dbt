use serde::Serialize;

/// A JSON formatter that matches Python's json.dumps() default style:
/// - Compact (no newlines or indentation)
/// - Space after colons (e.g., `{"key": "value"}` instead of `{"key":"value"}`)
/// - Space after commas (e.g., `{"a": 1, "b": 2}` instead of `{"a": 1,"b": 2}`)
/// - `ensure_ascii=True` (e.g., `"\u00e9"` instead of `"é"`)
struct PythonStyleFormatter;

impl serde_json::ser::Formatter for PythonStyleFormatter {
    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        writer.write_all(b": ")
    }

    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        for c in fragment.chars() {
            if (' '..='~').contains(&c) {
                writer.write_all(&[c as u8])?;
            } else {
                // Python escapes DEL too, and astral characters as UTF-16 surrogate pairs.
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}

/// Serialize a value to JSON string with Python-style formatting (space after colon)
pub(crate) fn to_json_string_python_style<T: Serialize>(
    value: &T,
) -> Result<String, serde_json::Error> {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, PythonStyleFormatter);
    value.serialize(&mut ser)?;
    // Safe because serde_json only produces valid UTF-8
    Ok(unsafe { String::from_utf8_unchecked(buf) })
}
