use std::convert::TryFrom;

use minijinja::arg_utils::ArgsIter;
use minijinja::value::{Rest, Value, ValueKind};
use minijinja::State;
use minijinja::{Error, ErrorKind};

#[cfg(feature = "datetime")]
mod datetime;

#[cfg(feature = "datetime")]
pub use self::datetime::*;

/// Returns a plural suffix if the value is not 1, '1', or an object of
/// length 1.
///
/// By default, the plural suffix is 's' and the singular suffix is
/// empty (''). You can specify a singular suffix as the first argument (or
/// `None`, for the default). You can specify a plural suffix as the second
/// argument (or `None`, for the default).
///
/// ```jinja
/// {{ users|length }} user{{ users|pluralize }}.
/// ```
///
/// ```jinja
/// {{ entities|length }} entit{{ entities|pluralize("y", "ies") }}.
/// ```
///
/// ```jinja
/// {{ platypuses|length }} platypus{{ platypuses|pluralize(None, "es") }}.
/// ```
pub fn pluralize(
    v: &Value,
    singular: Option<Value>,
    plural: Option<Value>,
) -> Result<Value, Error> {
    let is_singular = match v.len() {
        Some(val) => val == 1,
        None => match i64::try_from(v.clone()) {
            Ok(val) => val == 1,
            Err(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!(
                        "Pluralize argument is not an integer, or a sequence / object with a \
                         length but of type {}",
                        v.kind()
                    ),
                ));
            }
        },
    };

    let (rv, default) = if is_singular {
        (singular.unwrap_or(Value::UNDEFINED), "")
    } else {
        (plural.unwrap_or(Value::UNDEFINED), "s")
    };

    if rv.is_undefined() || rv.is_none() {
        Ok(Value::from(default))
    } else {
        Ok(rv)
    }
}

/// Chooses a random element from a sequence or string.
///
/// The random number generated can be seeded with the `RAND_SEED`
/// global context variable.
///
/// ```jinja
/// {{ [1, 2, 3, 4]|random }}
/// ```
#[cfg(feature = "rand")]
#[cfg_attr(docsrs, doc(cfg(feature = "rand")))]
pub fn random(state: &minijinja::State, seq: &Value) -> Result<Value, Error> {
    use crate::globals::get_rng;
    use minijinja::value::ValueKind;
    use rand::Rng;

    if matches!(
        seq.kind(),
        ValueKind::Seq | ValueKind::String | ValueKind::Iterable
    ) {
        let len = seq.len().unwrap_or(0);
        let idx = get_rng(state).gen_range(0..len);
        seq.get_item_by_index(idx)
    } else {
        Err(Error::new(
            ErrorKind::InvalidOperation,
            "can only select random elements from sequences",
        ))
    }
}

/// Formats the value like a "human-readable" file size.
///
/// For example. 13 kB, 4.1 MB, 102 Bytes, etc.  Per default decimal prefixes are
/// used (Mega, Giga, etc.),  if the second parameter is set to true
/// the binary prefixes are used (Mebi, Gibi).
pub fn filesizeformat(value: f64, binary: Option<bool>) -> String {
    const BIN_PREFIXES: &[&str] = &["KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    const SI_PREFIXES: &[&str] = &["kB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
    let (prefixes, base) = if binary.unwrap_or(false) {
        (BIN_PREFIXES, 1024.0)
    } else {
        (SI_PREFIXES, 1000.0)
    };

    if value == 1.0 {
        return "1 Byte".into();
    }
    let (sign, value) = if value < 0.0 {
        ("-", -value)
    } else {
        ("", value)
    };

    if value < base {
        format!("{sign}{value} Bytes")
    } else {
        for (idx, prefix) in prefixes.iter().enumerate() {
            let unit = base.powf(idx as f64 + 2.0);
            if value < unit || idx == prefixes.len() - 1 {
                return format!("{}{:.1} {}", sign, base * value / unit, prefix);
            }
        }
        unreachable!();
    }
}

/// Returns a truncated copy of the string.
///
/// The string will be truncated to the specified length, with an ellipsis
/// appended if truncation occurs. By default, the filter tries to preserve
/// whole words.
///
/// ```jinja
/// {{ "Hello World"|truncate(5) }}
/// {{ "Hello World"|truncate(length=5) }}
/// ```
///
/// The filter accepts parameters as positional or keyword arguments:
/// * `length`: maximum length of the output string (defaults to 255)
/// * `killwords`: set to `true` if you want to cut text exactly at length; if `false`,
///   the filter will preserve last word (defaults to `false`)
/// * `end`: if you want a specific ellipsis sign you can specify it (defaults to "...")
/// * `leeway`: determines the tolerance margin before truncation occurs (defaults to 5)
///
/// The truncation only occurs if the string length exceeds both the specified
/// length and the leeway margin combined. This means that if a string is just
/// slightly longer than the target length (within the leeway value), it will
/// be left unmodified.
///
/// When `killwords` is set to false (default behavior), the function ensures
/// that words remain intact by finding the last complete word that fits within
/// the length limit. This prevents words from being cut in the middle and
/// maintains text readability.
///
/// The specified length parameter is inclusive of the end string (ellipsis).
/// For example, with a length of 5 and the default ellipsis "...", only 2
/// characters from the original string will be preserved.
///
/// # Example with all attributes
/// ```jinja
/// {{ "Hello World"|truncate(
///     length=5,
///     killwords=true,
///     end='...',
///     leeway=2
/// ) }}
/// ```
pub fn truncate(state: &State, value: &Value, args: Rest<Value>) -> Result<String, Error> {
    let iter = ArgsIter::new("truncate", &[], &args);
    let length = iter.next_kwarg::<Option<usize>>("length")?.unwrap_or(255);
    let killwords = iter
        .next_kwarg::<Option<bool>>("killwords")?
        .unwrap_or_default();
    let end = iter.next_kwarg::<Option<&str>>("end")?.unwrap_or("...");
    let leeway = iter
        .next_kwarg::<Option<usize>>("leeway")?
        .unwrap_or_else(|| {
            state
                .lookup("TRUNCATE_LEEWAY", &[])
                .and_then(|x| usize::try_from(x).ok())
                .unwrap_or(5)
        });
    iter.finish()?;

    if matches!(value.kind(), ValueKind::None | ValueKind::Undefined) {
        return Ok("".into());
    }

    let s = value.as_str().ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("expected string, got {}", value.kind()),
        )
    })?;

    let end_len = end.chars().count();
    if length < end_len {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("expected length >= {end_len}, got {length}"),
        ));
    }

    if s.chars().count() <= length + leeway {
        return Ok(s.to_string());
    }

    let trunc_pos = length - end_len;
    let truncated = if killwords {
        s.chars().take(trunc_pos).collect::<String>()
    } else {
        let chars: Vec<char> = s.chars().take(trunc_pos).collect();
        match chars.iter().rposition(|&c| c == ' ') {
            Some(last_space) => chars[..last_space].iter().collect(),
            None => chars.iter().collect(),
        }
    };
    let mut result = String::with_capacity(truncated.len() + end.len());
    result.push_str(&truncated);
    result.push_str(end);
    Ok(result)
}

/// Counts the words in a string.
///
/// ```jinja
/// {{ "Hello world!"|wordcount }}
/// ```
#[cfg(feature = "wordcount")]
#[cfg_attr(docsrs, doc(cfg(feature = "wordcount")))]
pub fn wordcount(value: &Value) -> Result<Value, Error> {
    use unicode_categories::UnicodeCategories;

    let s = value.as_str().unwrap_or_default();
    let mut count: u32 = 0;
    let mut in_word = false;

    // Iterate through characters, counting transitions from non-word to word chars
    for c in s.chars() {
        let is_word_char = c.is_letter() || c.is_numeric() || c == '_';
        if is_word_char && !in_word {
            count += 1;
            in_word = true;
        } else if !is_word_char {
            in_word = false;
        }
    }

    Ok(Value::from(count))
}

/// Wrap a string to the given width.
///
/// By default this filter is not unicode aware (feature = `wordwrap`) but when the unicode
/// feature is enabled (`unicode_wordwrap`) then it becomes so.  It's implemented on top of
/// the `textwrap` crate.
///
/// **Keyword arguments:**
///
/// - `width`: Maximum length of wrapped lines (default: 79)
/// - `break_long_words`: If a word is longer than width, break it across lines (default: true)
/// - `break_on_hyphens`: If a word contains hyphens, it may be split across lines (default: true)
/// - `wrapstring`: String to join each wrapped line (default: newline)
#[cfg(feature = "wordwrap")]
#[cfg_attr(docsrs, doc(any(cfg(feature = "wordwrap"), cfg = "unicode_wordwrap")))]
pub fn wordwrap(value: &Value, args: Rest<Value>) -> Result<Value, Error> {
    use textwrap::{wrap, Options as WrapOptions, WordSplitter};

    // Jinja2 positional order: width, break_long_words, wrapstring, break_on_hyphens
    let iter = ArgsIter::new("wordwrap", &[], &args);
    let width = iter.next_kwarg::<Option<usize>>("width")?.unwrap_or(79);
    let break_long_words = iter
        .next_kwarg::<Option<bool>>("break_long_words")?
        .unwrap_or(true);
    let wrapstring = iter
        .next_kwarg::<Option<&str>>("wrapstring")?
        .unwrap_or("\n");
    let break_on_hyphens = iter
        .next_kwarg::<Option<bool>>("break_on_hyphens")?
        .unwrap_or(true);
    iter.finish()?;

    let s = value.as_str().unwrap_or_default();

    let mut options = WrapOptions::new(width).break_words(break_long_words);

    if break_on_hyphens {
        options = options.word_splitter(WordSplitter::HyphenSplitter);
    }

    // Handle empty/whitespace-only input
    if s.trim().is_empty() {
        return Ok(Value::from(""));
    }

    // Process paragraphs sequentially into final string
    Ok(Value::from(s.lines().enumerate().fold(
        String::new(),
        |mut acc, (i, p)| {
            if i > 0 {
                acc.push_str(wrapstring);
            }
            if !p.trim().is_empty() {
                // Wrap the paragraph and join with wrapstring
                let wrapped = wrap(p, &options);
                for (j, line) in wrapped.iter().enumerate() {
                    if j > 0 {
                        acc.push_str(wrapstring);
                    }
                    acc.push_str(line);
                }
            }
            acc
        },
    )))
}
