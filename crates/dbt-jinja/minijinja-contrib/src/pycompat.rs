use std::collections::BTreeSet;
use std::iter::zip;

use minijinja::arg_utils::ArgsIter;
use minijinja::tuple;
use minijinja::value::mutable_map::MutableMap;
use minijinja::value::mutable_vec::MutableVec;
use minijinja::value::{from_args, ValueKind, ValueMap};
use minijinja::{Error, ErrorKind, State, Value};
use regex::Regex;

/// An unknown method callback implementing python methods on primitives.
///
/// This implements a lot of Python methods on basic types so that the
/// compatibility with Jinja2 templates improves.
///
/// ```
/// use minijinja::Environment;
/// use minijinja_contrib::pycompat::unknown_method_callback;
///
/// let mut env = Environment::new();
/// env.set_unknown_method_callback(unknown_method_callback);
/// ```
///
/// Today the following methods are implemented:
///
/// * `dict.get`
/// * `dict.items`
/// * `dict.keys`
/// * `dict.meta_get`
/// * `dict.values`
/// * `dict.to_dict`
/// * `list.count`
/// * `list.index`
/// * `list.union`
/// * `str.capitalize`
/// * `str.casefold`
/// * `str.center`
/// * `str.count`
/// * `str.endswith`
/// * `str.expandtabs`
/// * `str.find`
/// * `str.format`
/// * `str.index`
/// * `str.isalnum`
/// * `str.isalpha`
/// * `str.isascii`
/// * `str.isdecimal`
/// * `str.isdigit`
/// * `str.isidentifier`
/// * `str.islower`
/// * `str.isnumeric`
/// * `str.isprintable`
/// * `str.isspace`
/// * `str.istitle`
/// * `str.isupper`
/// * `str.join`
/// * `str.ljust`
/// * `str.lower`
/// * `str.lstrip`
/// * `str.maketrans`
/// * `str.partition`
/// * `str.removeprefix`
/// * `str.removesuffix`
/// * `str.replace`
/// * `str.rfind`
/// * `str.rindex`
/// * `str.rjust`
/// * `str.rpartition`
/// * `str.rsplit`
/// * `str.rstrip`
/// * `str.split`
/// * `str.splitlines`
/// * `str.startswith`
/// * `str.strip`
/// * `str.swapcase`
/// * `str.title`
/// * `str.translate`
/// * `str.upper`
/// * `str.zfill`
#[cfg_attr(docsrs, doc(cfg(feature = "pycompat")))]
pub fn unknown_method_callback(
    _state: &State,
    value: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value, Error> {
    match value.kind() {
        ValueKind::String => string_methods(value, method, args),
        ValueKind::Undefined => undefined_methods(method, args),
        ValueKind::Map => map_methods(value, method, args),
        ValueKind::Seq => seq_methods(value, method, args),
        ValueKind::Number => number_methods(value, method, args),
        _ => Err(Error::from(ErrorKind::UnknownMethod)),
    }
}

fn undefined_methods(method: &str, args: &[Value]) -> Result<Value, Error> {
    match method {
        // Mirrors dbt-common's `Undefined.get(key, default=None)`: dbt
        // macros commonly probe an undefined `config`-like object with
        // `.get('key', default)`, and that must not fail. Every other
        // method call on an undefined value still fails with UnknownMethod
        // so the engine can name the undefined receiver in the error (see
        // `name_undefined_receiver` in vm/mod.rs).
        "get" => {
            let (_key, default): (&Value, Option<&Value>) = from_args(args)?;
            Ok(default.cloned().unwrap_or(Value::UNDEFINED))
        }
        _ => Err(Error::from(ErrorKind::UnknownMethod)),
    }
}

#[allow(clippy::cognitive_complexity)]
fn string_methods(value: &Value, method: &str, args: &[Value]) -> Result<Value, Error> {
    let s = match value.as_str() {
        Some(s) => s,
        None => return Err(Error::from(ErrorKind::UnknownMethod)),
    };

    match method {
        "upper" => {
            let () = from_args(args)?;
            Ok(Value::from(s.to_uppercase()))
        }
        "lower" => {
            let () = from_args(args)?;
            Ok(Value::from(s.to_lowercase()))
        }
        "islower" => {
            let () = from_args(args)?;
            Ok(Value::from(s.chars().all(|x| x.is_lowercase())))
        }
        "isupper" => {
            let () = from_args(args)?;
            Ok(Value::from(s.chars().all(|x| x.is_uppercase())))
        }
        "isspace" => {
            let () = from_args(args)?;
            Ok(Value::from(s.chars().all(|x| x.is_whitespace())))
        }
        "isdigit" | "isnumeric" => {
            // this is not a perfect mapping to what Python does, but
            // close enough for most uses in templates.
            let () = from_args(args)?;
            Ok(Value::from(s.chars().all(|x| x.is_numeric())))
        }
        "isalnum" => {
            let () = from_args(args)?;
            Ok(Value::from(s.chars().all(|x| x.is_alphanumeric())))
        }
        "isalpha" => {
            let () = from_args(args)?;
            Ok(Value::from(s.chars().all(|x| x.is_alphabetic())))
        }
        "isascii" => {
            let () = from_args(args)?;
            Ok(Value::from(s.is_ascii()))
        }
        "isidentifier" => {
            let () = from_args(args)?;
            let is_id = !s.is_empty() && {
                let mut chars = s.chars();
                // SAFETY: s is not empty
                let first = chars.next().unwrap();
                (first.is_alphabetic() || first == '_')
                    && chars.all(|c| c.is_alphanumeric() || c == '_')
            };
            Ok(Value::from(is_id))
        }
        "strip" => {
            let (chars,): (Option<&str>,) = from_args(args)?;
            Ok(Value::from(if let Some(chars) = chars {
                s.trim_matches(&chars.chars().collect::<Vec<_>>()[..])
            } else {
                s.trim()
            }))
        }
        "lstrip" => {
            let (chars,): (Option<&str>,) = from_args(args)?;
            Ok(Value::from(if let Some(chars) = chars {
                s.trim_start_matches(&chars.chars().collect::<Vec<_>>()[..])
            } else {
                s.trim_start()
            }))
        }
        "rstrip" => {
            let (chars,): (Option<&str>,) = from_args(args)?;
            Ok(Value::from(if let Some(chars) = chars {
                s.trim_end_matches(&chars.chars().collect::<Vec<_>>()[..])
            } else {
                s.trim_end()
            }))
        }
        "replace" => {
            let (old, new, count): (&str, &str, Option<i32>) = from_args(args)?;
            let count = count.unwrap_or(-1);
            Ok(Value::from(if count < 0 {
                s.replace(old, new)
            } else {
                s.replacen(old, new, count as usize)
            }))
        }
        "title" => {
            let () = from_args(args)?;
            // one shall not call into these filters.  However we consider ourselves
            // privileged.
            Ok(Value::from(minijinja::filters::title(s.into())))
        }
        "translate" => {
            let (table,): (&Value,) = from_args(args)?;
            if table.kind() != ValueKind::Map {
                // only support dictionaries
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "translate() argument must be a dict".to_string(),
                ));
            }
            // SAFETY: table kind is Map
            let obj = table.as_object().unwrap();
            let mut result = String::new();
            for c in s.chars() {
                let cp = c as u32 as f64;
                let key = Value::from(cp);
                if let Some(val) = obj.get_value(&key) {
                    if val.is_none() {
                        // delete
                        continue;
                    } else if let Some(new_cp) = val.as_i64() {
                        if let Some(new_c) = char::from_u32(new_cp as u32) {
                            result.push(new_c);
                        } else {
                            return Err(Error::new(
                                ErrorKind::InvalidOperation,
                                format!("invalid code point: {new_cp}"),
                            ));
                        }
                    } else if let Some(new_c) = val.as_str() {
                        if new_c.len() != 1 {
                            return Err(Error::new(
                                ErrorKind::InvalidOperation,
                                "translate table values must be a single character string"
                                    .to_string(),
                            ));
                        }
                        let new_c = new_c.chars().next().unwrap();
                        result.push(new_c);
                    } else {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            "translate table values must be integers, strings, or None".to_string(),
                        ));
                    }
                } else {
                    result.push(c);
                }
            }
            Ok(Value::from(result))
        }
        "split" => {
            let (sep, maxsplits) = from_args(args)?;
            // one shall not call into these filters.  However we consider ourselves
            // privileged.
            Ok(Value::from_object(MutableVec::from(
                minijinja::filters::split(s.into(), sep, maxsplits)
                    .try_iter()?
                    .collect::<Vec<Value>>(),
            )))
        }
        "splitlines" => {
            let (keepends,): (Option<bool>,) = from_args(args)?;
            if !keepends.unwrap_or(false) {
                Ok(Value::from_object(MutableVec::from(
                    s.lines().map(Value::from).collect::<Vec<Value>>(),
                )))
            } else {
                let mut rv = Vec::new();
                let mut rest = s;
                while let Some(offset) = rest.find('\n') {
                    rv.push(Value::from(&rest[..offset + 1]));
                    rest = &rest[offset + 1..];
                }
                if !rest.is_empty() {
                    rv.push(Value::from(rest));
                }
                Ok(Value::from_object(MutableVec::from(rv)))
            }
        }
        "capitalize" => {
            let () = from_args(args)?;
            // one shall not call into these filters.  However we consider ourselves
            // privileged.
            Ok(Value::from(minijinja::filters::capitalize(s.into())))
        }
        "count" => {
            let (what,): (&str,) = from_args(args)?;
            let mut c = 0;
            let mut rest = s;
            while let Some(offset) = rest.find(what) {
                c += 1;
                rest = &rest[offset + what.len()..];
            }
            Ok(Value::from(c))
        }
        "find" => {
            let (what,): (&str,) = from_args(args)?;
            Ok(Value::from(match s.find(what) {
                Some(x) => x as i64,
                None => -1,
            }))
        }
        "rfind" => {
            let (what,): (&str,) = from_args(args)?;
            Ok(Value::from(match s.rfind(what) {
                Some(x) => x as i64,
                None => -1,
            }))
        }
        "index" => {
            let (sub, start, end): (&str, Option<i64>, Option<i64>) = from_args(args)?;
            let len = s.len() as i64;
            let start_idx = start.unwrap_or(0);
            let start_idx = if start_idx < 0 {
                (len + start_idx).max(0) as usize
            } else {
                start_idx.min(len) as usize
            };
            let end_idx = end.unwrap_or(len);
            let end_idx = if end_idx < 0 {
                (len + end_idx).max(0) as usize
            } else {
                end_idx.min(len) as usize
            };
            if let Some(pos) = s[start_idx..end_idx].find(sub) {
                Ok(Value::from((start_idx + pos) as i64))
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("substring '{sub}' not found"),
                ))
            }
        }
        "startswith" => {
            let (prefix,): (&Value,) = from_args(args)?;
            if let Some(prefix) = prefix.as_str() {
                Ok(Value::from(s.starts_with(prefix)))
            } else if matches!(prefix.kind(), ValueKind::Iterable | ValueKind::Seq) {
                for prefix in prefix.try_iter()? {
                    if s.starts_with(prefix.as_str().ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            format!(
                                "tuple for startswith must contain only strings, not {}",
                                prefix.kind()
                            ),
                        )
                    })?) {
                        return Ok(Value::from(true));
                    }
                }
                Ok(Value::from(false))
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!(
                        "startswith argument must be string or a tuple of strings, not {}",
                        prefix.kind()
                    ),
                ))
            }
        }
        "endswith" => {
            let (suffix,): (&Value,) = from_args(args)?;
            if let Some(suffix) = suffix.as_str() {
                Ok(Value::from(s.ends_with(suffix)))
            } else if matches!(suffix.kind(), ValueKind::Iterable | ValueKind::Seq) {
                for suffix in suffix.try_iter()? {
                    if s.ends_with(suffix.as_str().ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            format!(
                                "tuple for endswith must contain only strings, not {}",
                                suffix.kind()
                            ),
                        )
                    })?) {
                        return Ok(Value::from(true));
                    }
                }
                Ok(Value::from(false))
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!(
                        "endswith argument must be string or a tuple of strings, not {}",
                        suffix.kind()
                    ),
                ))
            }
        }
        "join" => {
            use std::fmt::Write;
            let (values,): (&Value,) = from_args(args)?;
            let mut rv = String::new();
            for (idx, value) in values.try_iter()?.enumerate() {
                if idx > 0 {
                    rv.push_str(s);
                }
                write!(rv, "{value}").ok();
            }
            Ok(Value::from(rv))
        }
        "format" => {
            let args = args.to_vec();
            let mut result = s.to_string();

            // Handle numbered placeholders {0}, {1}, etc
            if Regex::new(r"\{\d+\}").unwrap().is_match(&result) {
                if result.contains("{}")
                    || Regex::new(r"\{[a-zA-Z_]\w*\}").unwrap().is_match(&result)
                {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "Cannot mix numbered placeholders with other placeholder types".to_string(),
                    ));
                }
                for (idx, value) in args.iter().enumerate() {
                    result = result.replace(&format!("{{{idx}}}"), &value.to_string());
                }
            }
            // Handle simple {} placeholders
            else if result.contains("{}") {
                // SAFETY: regex pattern is a valid constant
                if Regex::new(r"\{[a-zA-Z_]\w*\}").unwrap().is_match(&result) {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "Cannot mix empty placeholders with named placeholders".to_string(),
                    ));
                }
                for arg in args.iter() {
                    result = result.replacen("{}", &arg.to_string(), 1);
                }
            }
            // Handle named placeholders {name}
            // SAFETY: regex pattern is a valid constant
            else if Regex::new(r"\{[a-zA-Z_]\w*\}").unwrap().is_match(&result) {
                if args.len() != 1 || args[0].kind() != ValueKind::Map {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "Named placeholders require a dictionary argument".to_string(),
                    ));
                }
                if let Some(obj) = args[0].as_object() {
                    if let Some(iter) = obj.try_iter_pairs() {
                        for (key, value) in iter {
                            result = result.replace(&format!("{{{key}}}"), &value.to_string());
                        }
                    }
                }
            }
            Ok(Value::from(result))
        }
        "zfill" => {
            let (width,): (usize,) = from_args(args)?;
            Ok(Value::from(format!("{s:0>width$}")))
        }
        "removesuffix" => {
            let (suffix,): (&str,) = from_args(args)?;
            Ok(Value::from(s.trim_end_matches(suffix)))
        }
        "partition" => {
            let (sep,): (&str,) = from_args(args)?;
            let this = s;
            let tuple = if let Some((head, tail)) = this.split_once(sep) {
                vec![Value::from(head), Value::from(sep), Value::from(tail)]
            } else {
                vec![Value::from(this), Value::from(""), Value::from("")]
            };
            Ok(Value::from(tuple))
        }
        "casefold" => {
            let () = from_args(args)?;
            // Python's casefold is more aggressive than lowercase for case-insensitive matching
            // For most use cases, to_lowercase() is close enough
            Ok(Value::from(s.to_lowercase()))
        }
        "center" => {
            let (width, fillchar): (usize, Option<&str>) = from_args(args)?;
            let fillchar = fillchar.unwrap_or(" ");
            if fillchar.chars().count() != 1 {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "The fill character must be exactly one character long".to_string(),
                ));
            }
            // SAFETY: fillchar.chars().count() == 1, so next() will return Some
            let fillchar = fillchar.chars().next().unwrap();
            let s_len = s.chars().count();
            if s_len >= width {
                Ok(Value::from(s))
            } else {
                let total_padding = width - s_len;
                let left_padding = total_padding / 2;
                let right_padding = total_padding - left_padding;
                let result = format!(
                    "{}{}{}",
                    fillchar.to_string().repeat(left_padding),
                    s,
                    fillchar.to_string().repeat(right_padding)
                );
                Ok(Value::from(result))
            }
        }
        "expandtabs" => {
            let (tabsize,): (Option<usize>,) = from_args(args)?;
            let tabsize = tabsize.unwrap_or(8);
            let mut result = String::new();
            let mut col = 0;
            for c in s.chars() {
                if c == '\t' {
                    let spaces = tabsize - (col % tabsize);
                    result.push_str(&" ".repeat(spaces));
                    col += spaces;
                } else if c == '\n' || c == '\r' {
                    result.push(c);
                    col = 0;
                } else {
                    result.push(c);
                    col += 1;
                }
            }
            Ok(Value::from(result))
        }
        "isdecimal" => {
            let () = from_args(args)?;
            // In Python, isdecimal is stricter than isdigit
            // It only accepts decimal number characters (0-9)
            Ok(Value::from(
                !s.is_empty() && s.chars().all(|x| x.is_ascii_digit()),
            ))
        }
        "isprintable" => {
            let () = from_args(args)?;
            // A character is printable if it's not a control character
            Ok(Value::from(s.chars().all(|c| !c.is_control() || c == '\t')))
        }
        "istitle" => {
            let () = from_args(args)?;
            if s.is_empty() {
                return Ok(Value::from(false));
            }
            let mut prev_is_cased = false;
            let mut has_cased = false;
            for c in s.chars() {
                if c.is_uppercase() {
                    if prev_is_cased {
                        return Ok(Value::from(false));
                    }
                    prev_is_cased = true;
                    has_cased = true;
                } else if c.is_lowercase() {
                    if !prev_is_cased {
                        return Ok(Value::from(false));
                    }
                    has_cased = true;
                } else {
                    prev_is_cased = false;
                }
            }
            Ok(Value::from(has_cased))
        }
        "ljust" => {
            let (width, fillchar): (usize, Option<&str>) = from_args(args)?;
            let fillchar = fillchar.unwrap_or(" ");
            if fillchar.chars().count() != 1 {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "The fill character must be exactly one character long".to_string(),
                ));
            }
            // SAFETY: fillchar.chars().count() == 1, so next() will return Some
            let fillchar = fillchar.chars().next().unwrap();
            let s_len = s.chars().count();
            if s_len >= width {
                Ok(Value::from(s))
            } else {
                let result = format!("{}{}", s, fillchar.to_string().repeat(width - s_len));
                Ok(Value::from(result))
            }
        }
        "removeprefix" => {
            let (prefix,): (&str,) = from_args(args)?;
            Ok(Value::from(s.strip_prefix(prefix).unwrap_or(s)))
        }
        "rindex" => {
            let (sub, start, end): (&str, Option<i64>, Option<i64>) = from_args(args)?;
            let len = s.len() as i64;
            let start_idx = start.unwrap_or(0);
            let start_idx = if start_idx < 0 {
                (len + start_idx).max(0) as usize
            } else {
                start_idx.min(len) as usize
            };
            let end_idx = end.unwrap_or(len);
            let end_idx = if end_idx < 0 {
                (len + end_idx).max(0) as usize
            } else {
                end_idx.min(len) as usize
            };
            if let Some(pos) = s[start_idx..end_idx].rfind(sub) {
                Ok(Value::from((start_idx + pos) as i64))
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("substring '{sub}' not found"),
                ))
            }
        }
        "rjust" => {
            let (width, fillchar): (usize, Option<&str>) = from_args(args)?;
            let fillchar = fillchar.unwrap_or(" ");
            if fillchar.chars().count() != 1 {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "The fill character must be exactly one character long".to_string(),
                ));
            }
            // SAFETY: fillchar.chars().count() == 1, so next() will return Some
            let fillchar = fillchar.chars().next().unwrap();
            let s_len = s.chars().count();
            if s_len >= width {
                Ok(Value::from(s))
            } else {
                let result = format!("{}{}", fillchar.to_string().repeat(width - s_len), s);
                Ok(Value::from(result))
            }
        }
        "rpartition" => {
            let (sep,): (&str,) = from_args(args)?;
            let this = s;
            let tuple = if let Some((head, tail)) = this.rsplit_once(sep) {
                vec![Value::from(head), Value::from(sep), Value::from(tail)]
            } else {
                vec![Value::from(""), Value::from(""), Value::from(this)]
            };
            Ok(Value::from(tuple))
        }
        "rsplit" => {
            let (sep, maxsplits): (Option<&str>, Option<i32>) = from_args(args)?;
            let maxsplits = maxsplits.unwrap_or(-1);

            let parts: Vec<Value> = if let Some(sep) = sep {
                if maxsplits < 0 {
                    // No limit - same as split() (returns left-to-right order)
                    s.split(sep).map(Value::from).collect()
                } else {
                    // Use rsplitn (splits from right) and reverse to get left-to-right order
                    let mut parts: Vec<Value> = s
                        .rsplitn((maxsplits + 1) as usize, sep)
                        .map(Value::from)
                        .collect();
                    parts.reverse();
                    parts
                }
            } else {
                // Split on whitespace
                if maxsplits < 0 {
                    s.split_whitespace().map(Value::from).collect()
                } else {
                    let parts: Vec<&str> = s.split_whitespace().collect();
                    if parts.len() <= maxsplits as usize {
                        parts.into_iter().map(Value::from).collect()
                    } else {
                        let split_point = parts.len() - maxsplits as usize;
                        let mut result: Vec<Value> =
                            vec![Value::from(parts[..split_point].join(" "))];
                        result.extend(parts[split_point..].iter().map(|&p| Value::from(p)));
                        result
                    }
                }
            };
            Ok(Value::from_object(MutableVec::from(parts)))
        }
        "swapcase" => {
            let () = from_args(args)?;
            let result: String = s
                .chars()
                .map(|c| {
                    if c.is_lowercase() {
                        c.to_uppercase().collect::<String>()
                    } else if c.is_uppercase() {
                        c.to_lowercase().collect::<String>()
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            Ok(Value::from(result))
        }
        // Reference: https://docs.python.org/3/library/stdtypes.html#str.maketrans
        "maketrans" => {
            // `maketrans(x: dict)`: If there is only one argument,
            // it must be a dictionary mapping Unicode ordinals
            // (integers) or characters (strings of length 1) to
            // Unicode ordinals, strings (of arbitrary lengths) or
            // None. Character keys will then be converted to
            // ordinals.
            if args.len() == 1 {
                let iter = ArgsIter::new("maketrans", &["x"], args);
                let mapping = iter.next_arg::<&Value>()?;
                iter.finish()?;

                let result = mapping
                    .try_iter()?
                    .map(|key| {
                        let value = mapping.get_item(&key).map_err(|_| {
                            Error::new(
                                ErrorKind::InvalidOperation,
                                "if you give only one argument to maketrans it must be a dict"
                                    .to_string(),
                            )
                        })?;

                        // Validate key type: must be a string of length 1 or integer
                        // Convert all keys to ordinals.
                        let key = if key.is_integer() {
                            key
                        } else if let Some(s) = key.as_str() {
                            if s.len() != 1 {
                                return Err(Error::new(
                                    ErrorKind::InvalidOperation,
                                    "string keys in translate table must be of length 1"
                                        .to_string(),
                                ));
                            }
                            // SAFETY: |s| = 1
                            let c = s.chars().next().unwrap();
                            Value::from(c as u32)
                        } else {
                            return Err(Error::new(
                                ErrorKind::InvalidOperation,
                                "keys in translate table must be strings or integers".to_string(),
                            ));
                        };

                        // Values are NOT validated in `maketrans`
                        Ok((key, value))
                    })
                    .collect::<Result<ValueMap, Error>>()?;
                Ok(Value::from(result))
            }
            // `maketrans(x: str, y: str)`: If there are two
            // arguments, they must be strings of equal length, and
            // in the resulting dictionary, each character in from
            // will be mapped to the character at the same position
            // in to. If there is a third argument, it must be a
            // string, whose characters will be mapped to None in
            // the result.
            else {
                let iter = ArgsIter::new("maketrans", &["x", "y", "z"], args);
                let from = iter.next_arg::<&str>()?;
                let to = iter.next_arg::<&str>()?;
                let remove = iter.next_arg::<Option<&str>>()?;
                iter.finish()?;
                if from.len() != to.len() {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "maketrans arguments must be strings of equal length".to_string(),
                    ));
                }
                // This might overshoot the capacity if there is overlap between from and remove.
                let mut result =
                    ValueMap::with_capacity(from.len() + remove.map_or(0, |r| r.len()));
                for (from_c, to_c) in zip(from.chars(), to.chars()) {
                    result.insert(
                        Value::from(from_c as u32 as f64),
                        Value::from(to_c as u32 as f64),
                    );
                }
                if let Some(remove) = remove {
                    for remove_c in remove.chars() {
                        result.insert(Value::from(remove_c as u32 as f64), Value::NONE);
                    }
                }
                Ok(Value::from(result))
            }
        }
        _ => Err(Error::from(ErrorKind::UnknownMethod)),
    }
}

fn map_methods(value: &Value, method: &str, args: &[Value]) -> Result<Value, Error> {
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => return Err(Error::from(ErrorKind::UnknownMethod)),
    };

    match method {
        "keys" => {
            let () = from_args(args)?;
            Ok(Value::make_object_iterable(obj.clone(), |obj| {
                match obj.try_iter() {
                    Some(iter) => iter,
                    None => Box::new(None.into_iter()),
                }
            }))
        }
        "values" => {
            let () = from_args(args)?;
            Ok(Value::make_object_iterable(obj.clone(), |obj| {
                match obj.try_iter_pairs() {
                    Some(iter) => Box::new(iter.map(|(_, v)| v)),
                    None => Box::new(None.into_iter()),
                }
            }))
        }
        "items" => {
            let () = from_args(args)?;
            Ok(Value::make_object_iterable(obj.clone(), |obj| {
                match obj.try_iter_pairs() {
                    Some(iter) => Box::new(iter.map(|(k, v)| Value::from(tuple![k, v]))),
                    None => Box::new(None.into_iter()),
                }
            }))
        }
        "get" => {
            let (key, default): (&Value, Option<&Value>) = from_args(args)?;
            Ok(match obj.get_value(key) {
                Some(value) if !value.is_none() => value,
                _ => default.cloned().unwrap_or_else(|| Value::from(())),
            })
        }
        "meta_get" => {
            let iter = ArgsIter::new("meta_get", &["key"], args);
            let key = iter.next_pos_arg_aliased::<&Value>(&["name"])?;
            let default = iter
                .next_kwarg::<Option<&Value>>("default")?
                .cloned()
                .unwrap_or_else(|| Value::from(()));
            iter.finish()?;

            Ok(obj
                .get_value(&Value::from("meta"))
                .and_then(|meta| meta.as_object().and_then(|meta| meta.get_value(key)))
                .unwrap_or(default))
        }
        "to_dict" => {
            let () = from_args(args)?;
            // to_dict() on a dict just returns the dict itself
            Ok(value.clone())
        }
        "copy" => {
            let () = from_args(args)?;
            let map = MutableMap::new();
            if let Some(iter) = obj.try_iter_pairs() {
                for (k, v) in iter {
                    map.insert(k, v);
                }
            }
            Ok(Value::from_object(map))
        }
        _ => Err(Error::from(ErrorKind::UnknownMethod)),
    }
}

fn seq_methods(value: &Value, method: &str, args: &[Value]) -> Result<Value, Error> {
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => return Err(Error::from(ErrorKind::UnknownMethod)),
    };

    match method {
        "count" => {
            let (what,): (&Value,) = from_args(args)?;
            Ok(Value::from(if let Some(iter) = obj.try_iter() {
                iter.filter(|x| x == what).count()
            } else {
                0
            }))
        }
        "__sub__" => {
            let (other,): (&Value,) = from_args(args)?;
            if other.kind() == ValueKind::Seq {
                let other_set = other.try_iter().unwrap().collect::<BTreeSet<_>>();
                let mut result = Vec::new();
                for item in obj.try_iter().unwrap() {
                    if !other_set.contains(&item) {
                        result.push(item.clone());
                    }
                }
                return Ok(Value::from_object(MutableVec::from(result)));
            }
            Err(Error::new(
                ErrorKind::InvalidOperation,
                "Cannot subtract non-sequence".to_string(),
            ))
        }
        "union" => {
            // Handle multiple arguments like Python's set.union(*others)
            let mut result_set = BTreeSet::new();

            // Add all items from the original sequence
            if let Some(iter) = obj.try_iter() {
                for item in iter {
                    result_set.insert(item);
                }
            }

            // Add all items from each argument sequence
            for arg in args {
                match arg.try_iter() {
                    Ok(iter) => {
                        for item in iter {
                            result_set.insert(item);
                        }
                    }
                    Err(_) => {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            "union() argument must be iterable",
                        ));
                    }
                }
            }

            // Convert result back to a sequence
            let result: Vec<Value> = result_set.into_iter().collect();
            Ok(Value::from_object(MutableVec::from(result)))
        }
        "index" => {
            let (what, start, end): (&Value, Option<i64>, Option<i64>) = from_args(args)?;
            if let Some(iter) = obj.try_iter() {
                let items: Vec<Value> = iter.collect();
                let len = items.len() as i64;

                // Handle negative indices and bounds
                let start_idx = start.unwrap_or(0);
                let start_idx = if start_idx < 0 {
                    (len + start_idx).max(0) as usize
                } else {
                    start_idx.min(len) as usize
                };

                let end_idx = end.unwrap_or(len);
                let end_idx = if end_idx < 0 {
                    (len + end_idx).max(0) as usize
                } else {
                    end_idx.min(len) as usize
                };

                // Search for the item in the specified range
                for (i, item) in items.iter().enumerate().skip(start_idx) {
                    if i >= end_idx {
                        break;
                    }
                    if item == what {
                        return Ok(Value::from(i as i64));
                    }
                }

                // Item not found - raise ValueError equivalent
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("{what} is not in list"),
                ))
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "Cannot index non-iterable".to_string(),
                ))
            }
        }
        // `pop` mutates the sequence in place. Mutable sequences (e.g.
        // `MutableVec`) already implement it via `Object::call_method`, so if
        // we get here the sequence isn't mutable and, like Python's tuple,
        // doesn't support `pop()` at all.
        _ => Err(Error::from(ErrorKind::UnknownMethod)),
    }
}

fn number_methods(value: &Value, method: &str, args: &[Value]) -> Result<Value, Error> {
    let i = value.as_i64().unwrap_or_default();
    match method {
        "strftime" => {
            // i is the timestamp in seconds
            let (format,): (&str,) = from_args(args)?;
            if let Some(dt) = chrono::DateTime::from_timestamp(i, 0) {
                let formatted = dt.format(format);
                Ok(Value::from(formatted.to_string()))
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "Invalid timestamp".to_string(),
                ))
            }
        }
        _ => Err(Error::from(ErrorKind::UnknownMethod)),
    }
}
