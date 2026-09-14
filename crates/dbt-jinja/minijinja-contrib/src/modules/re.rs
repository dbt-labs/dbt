//! A mini re-like module for MiniJinja, intended to mirror Python's `re` module behavior.
//!
//! This module provides functions such as `compile`, `match`, `search`, `fullmatch`,
//! `findall`, `split`, `sub`, etc., using Rust's `regex` crate under the hood. While
//! this is only a partial implementation of Python's `re` spec, it demonstrates the
//! pattern-oriented usage consistent with MiniJinja's function/value approach.

use fancy_regex::{Captures, Expander, Regex}; // like python regex, fancy_regex supports lookadheds/lookbehinds
use indexmap::IndexMap;
use minijinja::{
    arg_utils::ArgsIter,
    value::{Enumerator, Object, ObjectRepr, ValueMap},
    Error, ErrorKind, Value,
};
use std::{collections::BTreeMap, fmt, sync::Arc};

// Python re flag values (matching CPython's enum values)
// https://docs.python.org/3/library/re.html#flags
const RE_NOFLAG: i64 = 0;
const RE_IGNORECASE: i64 = 2;
const RE_LOCALE: i64 = 4;
const RE_MULTILINE: i64 = 8;
const RE_DOTALL: i64 = 16;
const RE_UNICODE: i64 = 32;
const RE_VERBOSE: i64 = 64;
const RE_ASCII: i64 = 256;

/// A Python `re.RegexFlag`-like object that renders as `re.FLAGNAME` and has an integer value.
#[derive(Debug, Clone)]
struct ReFlag {
    name: &'static str,
    value: i64,
}

impl Object for ReFlag {
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result
    where
        Self: Sized + 'static,
    {
        write!(f, "re.{}", self.name)
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        // Support `| int` filter by exposing the numeric value through __int__
        if key.as_str() == Some("__int__") {
            Some(Value::from(self.value))
        } else {
            None
        }
    }
}

/// Extract the integer flags value from a `Value` which may be a `ReFlag` object or a plain int.
fn extract_flags(val: &Value) -> i64 {
    if let Some(obj) = val.as_object() {
        if let Some(flag) = obj.downcast_ref::<ReFlag>() {
            return flag.value;
        }
    }
    val.as_i64().unwrap_or(0)
}

/// Build an inline regex flag prefix (e.g. `(?i)`, `(?imsx)`) from the integer flags bitmask.
///
/// Only flags that map to regex inline modifiers are emitted. `UNICODE` is already the default
/// in Rust's regex engine; `ASCII`, `LOCALE`, and `DEBUG` have no regex-level equivalent and
/// are silently ignored here.
fn flags_to_inline_prefix(flags: i64) -> String {
    if flags == 0 {
        return String::new();
    }
    let mut prefix = String::from("(?");
    if flags & RE_IGNORECASE != 0 {
        prefix.push('i');
    }
    if flags & RE_MULTILINE != 0 {
        prefix.push('m');
    }
    if flags & RE_DOTALL != 0 {
        prefix.push('s');
    }
    if flags & RE_VERBOSE != 0 {
        prefix.push('x');
    }
    prefix.push(')');
    prefix
}

/// Create a namespace with `re`-like functions for pattern matching.
pub fn create_re_namespace() -> BTreeMap<String, Value> {
    let mut re_module = BTreeMap::new();

    // Python-like top-level functions:
    re_module.insert("compile".to_string(), Value::from_function(re_compile));
    re_module.insert("match".to_string(), Value::from_function(re_match));
    re_module.insert("search".to_string(), Value::from_function(re_search));
    re_module.insert("fullmatch".to_string(), Value::from_function(re_fullmatch));
    re_module.insert("findall".to_string(), Value::from_function(re_findall));
    re_module.insert("split".to_string(), Value::from_function(re_split));
    re_module.insert("sub".to_string(), Value::from_function(re_sub));
    re_module.insert("escape".to_string(), Value::from_function(re_escape));

    // Flag constants (matching Python's re module)
    // https://docs.python.org/3/library/re.html#flags
    re_module.insert(
        "NOFLAG".to_string(),
        Value::from_object(ReFlag {
            name: "NOFLAG",
            value: RE_NOFLAG,
        }),
    );

    let ignorecase = Value::from_object(ReFlag {
        name: "IGNORECASE",
        value: RE_IGNORECASE,
    });
    re_module.insert("IGNORECASE".to_string(), ignorecase.clone());
    re_module.insert("I".to_string(), ignorecase);

    let locale = Value::from_object(ReFlag {
        name: "LOCALE",
        value: RE_LOCALE,
    });
    re_module.insert("LOCALE".to_string(), locale.clone());
    re_module.insert("L".to_string(), locale);

    let multiline = Value::from_object(ReFlag {
        name: "MULTILINE",
        value: RE_MULTILINE,
    });
    re_module.insert("MULTILINE".to_string(), multiline.clone());
    re_module.insert("M".to_string(), multiline);

    let dotall = Value::from_object(ReFlag {
        name: "DOTALL",
        value: RE_DOTALL,
    });
    re_module.insert("DOTALL".to_string(), dotall.clone());
    re_module.insert("S".to_string(), dotall);

    let unicode = Value::from_object(ReFlag {
        name: "UNICODE",
        value: RE_UNICODE,
    });
    re_module.insert("UNICODE".to_string(), unicode.clone());
    re_module.insert("U".to_string(), unicode);

    let verbose = Value::from_object(ReFlag {
        name: "VERBOSE",
        value: RE_VERBOSE,
    });
    re_module.insert("VERBOSE".to_string(), verbose.clone());
    re_module.insert("X".to_string(), verbose);

    let ascii = Value::from_object(ReFlag {
        name: "ASCII",
        value: RE_ASCII,
    });
    re_module.insert("ASCII".to_string(), ascii.clone());
    re_module.insert("A".to_string(), ascii);

    re_module
}

/// Compile the given pattern into a RegexObject.
///
/// Python signature: re.compile(pattern, flags=0)
fn re_compile(args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("compile", &["pattern"], args);
    let pattern_val: &Value = iter.next_arg()?;
    let pattern_str = pattern_val.to_string();
    let flags = iter
        .next_kwarg::<Option<&Value>>("flags")?
        .map(extract_flags)
        .unwrap_or(0);
    iter.finish()?;

    let compiled = compile_pattern(&pattern_str, flags)?;
    let pattern = Pattern::new(&pattern_str, *compiled);
    Ok(Value::from_object(pattern))
}

#[derive(Debug, Clone)]
pub struct Pattern {
    raw: String,
    compiled: Regex,
}

impl Pattern {
    pub fn new(raw: &str, compiled: Regex) -> Self {
        Self {
            raw: raw.to_string(),
            compiled,
        }
    }
}

impl Object for Pattern {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "pattern" => Some(Value::from(self.raw.clone())),
            _ => None,
        }
    }

    fn call_method(
        self: &std::sync::Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        let pattern_val = Value::from_object(self.as_ref().clone());
        match method {
            "match" | "search" | "fullmatch" | "findall" => {
                let method_name = format!("Pattern.{method}");
                let iter = ArgsIter::new(&method_name, &["string"], args);
                let string: &Value = iter.next_arg()?;
                iter.finish()?;
                let pair = [pattern_val, string.clone()];
                match method {
                    "match" => re_match(&pair),
                    "search" => re_search(&pair),
                    "fullmatch" => re_fullmatch(&pair),
                    "findall" => re_findall(&pair),
                    _ => unreachable!(),
                }
            }
            "split" => {
                let iter = ArgsIter::new("Pattern.split", &["string"], args);
                let string: &Value = iter.next_arg()?;
                let maxsplit = iter.next_kwarg::<Option<i64>>("maxsplit")?.unwrap_or(0);
                iter.finish()?;
                if maxsplit != 0 {
                    let a = [pattern_val, string.clone(), Value::from(maxsplit)];
                    re_split(&a)
                } else {
                    let a = [pattern_val, string.clone()];
                    re_split(&a)
                }
            }
            "sub" => {
                let iter = ArgsIter::new("Pattern.sub", &["repl", "string"], args);
                let repl: &Value = iter.next_arg()?;
                let string: &Value = iter.next_arg()?;
                let count = iter.next_kwarg::<Option<i64>>("count")?.unwrap_or(0);
                iter.finish()?;
                if count != 0 {
                    let a = [
                        pattern_val,
                        repl.clone(),
                        string.clone(),
                        Value::from(count),
                    ];
                    re_sub(&a)
                } else {
                    let a = [pattern_val, repl.clone(), string.clone()];
                    re_sub(&a)
                }
            }
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("Pattern object has no method named '{method}'"),
            )),
        }
    }
}

/// Python `re.match(pattern, string, flags=0)`.
/// Checks for a match only at the beginning of the string.
fn re_match(args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("match", &["pattern", "string"], args);
    let pattern: &Value = iter.next_arg()?;
    let string: &Value = iter.next_arg()?;
    let flags = iter
        .next_kwarg::<Option<&Value>>("flags")?
        .map(extract_flags)
        .unwrap_or(0);
    iter.finish()?;
    let args_pair = [pattern.clone(), string.clone()];
    let (regex, text) = get_or_compile_regex_and_text_with_flags(&args_pair, flags)?;
    let raw_pattern = regex.as_str().to_string();
    let input_string = text.to_string();

    let mut pattern = String::from(r"\A");
    pattern.push_str(&raw_pattern);

    let start_anchored = Regex::new(&pattern).map_err(|e| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("Failed to compile regex: {e}"),
        )
    })?;

    if let Ok(Some(captures)) = start_anchored.captures(text) {
        let groups: Vec<(Value, Option<Span>)> = captures
            .iter()
            .map(|m| {
                m.map(|m| (Value::from(m.as_str()), Some((m.start(), m.end()))))
                    .unwrap_or((Value::NONE, None))
            })
            .collect();
        let names = start_anchored.capture_names();
        let named_groups: IndexMap<String, usize> = IndexMap::from_iter(
            names
                .enumerate()
                .filter_map(|(idx, name)| name.map(|name| (name.to_string(), idx))),
        );
        let capture = Capture::new(groups, named_groups, input_string, raw_pattern);
        Ok(Value::from_object(capture))
    } else {
        Ok(Value::NONE)
    }
}

/// Python `re.search(pattern, string, flags=0)`.
/// Searches through the entire string for the first match.
fn re_search(args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("search", &["pattern", "string"], args);
    let pattern: &Value = iter.next_arg()?;
    let string: &Value = iter.next_arg()?;
    let flags = iter
        .next_kwarg::<Option<&Value>>("flags")?
        .map(extract_flags)
        .unwrap_or(0);
    iter.finish()?;
    let args_pair = [pattern.clone(), string.clone()];
    let (regex, text) = get_or_compile_regex_and_text_with_flags(&args_pair, flags)?;
    let raw_pattern = regex.as_str().to_string();
    let input_string = text.to_string();

    if let Ok(Some(captures)) = regex.captures(text) {
        let groups: Vec<(Value, Option<Span>)> = captures
            .iter()
            .map(|m| {
                m.map(|m| (Value::from(m.as_str()), Some((m.start(), m.end()))))
                    .unwrap_or((Value::NONE, None))
            })
            .collect();
        let names = regex.capture_names();
        let named_groups: IndexMap<String, usize> = IndexMap::from_iter(
            names
                .enumerate()
                .filter_map(|(idx, name)| name.map(|name| (name.to_string(), idx))),
        );
        let capture = Capture::new(groups, named_groups, input_string, raw_pattern);
        Ok(Value::from_object(capture))
    } else {
        Ok(Value::NONE)
    }
}

/// Python `re.fullmatch(pattern, string, flags=0)`.
/// Matches the entire string against the pattern (like `^pattern$`).
fn re_fullmatch(args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("fullmatch", &["pattern", "string"], args);
    let pattern: &Value = iter.next_arg()?;
    let string: &Value = iter.next_arg()?;
    let flags = iter
        .next_kwarg::<Option<&Value>>("flags")?
        .map(extract_flags)
        .unwrap_or(0);
    iter.finish()?;
    let args_pair = [pattern.clone(), string.clone()];
    let (regex, text) = get_or_compile_regex_and_text_with_flags(&args_pair, flags)?;
    match regex.find(text) {
        Ok(Some(m)) if m.start() == 0 && m.end() == text.len() => {
            Ok(match_obj_to_list(&regex, text, m.start(), m.end()))
        }
        _ => Ok(Value::from(None::<Value>)),
    }
}

/// Python `re.findall(pattern, string, flags=0)`.
/// Returns all non-overlapping matches of pattern in string, as a list of strings or
/// list of tuples if groups exist.
fn re_findall(args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("findall", &["pattern", "string"], args);
    let pattern: &Value = iter.next_arg()?;
    let string: &Value = iter.next_arg()?;
    let flags = iter
        .next_kwarg::<Option<&Value>>("flags")?
        .map(extract_flags)
        .unwrap_or(0);
    iter.finish()?;
    let args_pair = [pattern.clone(), string.clone()];
    let (regex, text) = get_or_compile_regex_and_text_with_flags(&args_pair, flags)?;
    let raw_pattern = regex.as_str().to_string();
    let input_string = text.to_string();

    let matches =
        regex
            .captures_iter(text)
            .map(|captures| {
                let captures =
                    captures.map_err(|err| Error::new(ErrorKind::RegexError, err.to_string()))?;
                Ok(match captures.len() {
                    1 => {
                        let full = captures.get(0).unwrap().as_str();
                        Value::from(full)
                    }
                    2 => {
                        let capture = captures.get(1).unwrap().as_str();
                        Value::from(capture)
                    }
                    _ => {
                        let groups: Vec<(Value, Option<Span>)> = captures
                            .iter()
                            .skip(1)
                            .map(|m| {
                                m.map(|m| (Value::from(m.as_str()), Some((m.start(), m.end()))))
                                    .unwrap_or((Value::NONE, None))
                            })
                            .collect();
                        let names = regex.capture_names();
                        let named_groups: IndexMap<String, usize> =
                            IndexMap::from_iter(names.enumerate().skip(1).filter_map(
                                |(idx, name)| name.map(|name| (name.to_string(), idx)),
                            ));
                        let capture = Capture::new_findall(
                            groups,
                            named_groups,
                            input_string.clone(),
                            raw_pattern.clone(),
                        );
                        Value::from_object(capture)
                    }
                })
            })
            .collect::<Result<Vec<Value>, Error>>()?;

    Ok(Value::from(matches))
}

/// Python `re.split(pattern, string, maxsplit=0, flags=0)`.
/// Split string by occurrences of pattern. If capturing groups are used,
/// those are included in the result.
fn re_split(args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("split", &["pattern", "string"], args);
    let pattern: &Value = iter.next_arg()?;
    let string: &Value = iter.next_arg()?;
    let maxsplit = iter.next_kwarg::<Option<i64>>("maxsplit")?.unwrap_or(0) as usize;
    let flags = iter
        .next_kwarg::<Option<&Value>>("flags")?
        .map(extract_flags)
        .unwrap_or(0);
    iter.finish()?;
    let args_pair = [pattern.clone(), string.clone()];
    let (regex, text) = get_or_compile_regex_and_text_with_flags(&args_pair, flags)?;

    let mut result = Vec::new();
    let mut last = 0;

    for (n, captures) in regex.captures_iter(text).enumerate() {
        if maxsplit != 0 && n >= maxsplit {
            break;
        }
        let captures =
            captures.map_err(|err| Error::new(ErrorKind::RegexError, err.to_string()))?;

        let full = captures.get(0).unwrap();
        result.push(Value::from(&text[last..full.start()]));

        for m in captures.iter().skip(1) {
            if let Some(m) = m {
                result.push(Value::from(m.as_str()));
            } else {
                result.push(Value::from(""));
            }
        }

        last = full.end();
    }

    if last <= text.len() {
        result.push(Value::from(&text[last..]));
    }

    Ok(Value::from(result))
}

/// Python `re.sub(pattern, repl, string, count=0, flags=0)`.
/// Return the string obtained by replacing the leftmost non-overlapping occurrences
/// of pattern in string by repl. If repl is a function, it is called for every match.
fn re_sub(args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("sub", &["pattern", "repl", "string"], args);
    let pattern: &Value = iter.next_arg()?;
    let repl: &Value = iter.next_arg()?;
    let string: &Value = iter.next_arg()?;
    let count = iter.next_kwarg::<Option<i64>>("count")?.unwrap_or(0);
    let flags = iter
        .next_kwarg::<Option<&Value>>("flags")?
        .map(extract_flags)
        .unwrap_or(0);
    iter.finish()?;

    let args_pair = [pattern.clone(), repl.clone()];
    let (regex, _text) = get_or_compile_regex_and_text_with_flags(&args_pair, flags)?;
    let repl_text = repl.to_string();
    let text_arg = &string.to_string();

    let expander = Expander::python();
    let mut nonempty_probe = None;
    let mut result = String::with_capacity(text_arg.len());
    let mut last_match = 0;
    let mut search_pos = 0;
    let mut replacements = 0;
    let mut force_nonempty_at = None;

    while search_pos <= text_arg.len() {
        if count != 0 && replacements >= count as usize {
            break;
        }

        let pending_nonempty = force_nonempty_at.take();
        let Some((captures, start, end)) = (if let Some(pos) = pending_nonempty {
            let probe = match nonempty_probe.as_ref() {
                Some(probe) => probe,
                None => {
                    nonempty_probe = Some(nonempty_regex(regex.as_str())?);
                    nonempty_probe.as_ref().unwrap()
                }
            };
            nonempty_captures_from_pos(probe, text_arg, pos)?
        } else {
            regex
                .captures_from_pos(text_arg, search_pos)
                .map_err(|err| Error::new(ErrorKind::RegexError, err.to_string()))?
                .map(|captures| {
                    let matched = captures.get(0).unwrap();
                    (captures, matched.start(), matched.end())
                })
        }) else {
            if let Some(pos) = pending_nonempty {
                search_pos = next_utf8_boundary(text_arg, pos);
                continue;
            }
            break;
        };

        result.push_str(&text_arg[last_match..start]);
        result.push_str(&expander.expansion(&repl_text, &captures));
        last_match = end;
        replacements += 1;

        if start == end {
            force_nonempty_at = Some(end);
        }
        search_pos = end;
    }

    result.push_str(&text_arg[last_match..]);
    Ok(Value::from(result))
}

/// Builds the same-start non-empty probe for `re.sub` empty-match handling.
fn nonempty_regex(pattern: &str) -> Result<Regex, Error> {
    let suffix_sep = if verbose_mode_active_at_end(pattern) {
        "\n"
    } else {
        ""
    };
    Regex::new(&format!(r"\G(?:{pattern}{suffix_sep})(?<!\G)"))
        .map_err(|err| Error::new(ErrorKind::RegexError, err.to_string()))
}

/// Finds a non-empty match that starts at `pos`.
fn nonempty_captures_from_pos<'a>(
    regex: &Regex,
    text: &'a str,
    pos: usize,
) -> Result<Option<(Captures<'a>, usize, usize)>, Error> {
    let Some(captures) = regex
        .captures_from_pos(text, pos)
        .map_err(|err| Error::new(ErrorKind::RegexError, err.to_string()))?
    else {
        return Ok(None);
    };
    let matched = captures.get(0).unwrap();
    if matched.start() == pos && matched.end() > pos {
        return Ok(Some((captures, pos, matched.end())));
    }

    Ok(None)
}

/// Returns true when `x`/verbose mode is still active after parsing `pattern`.
fn verbose_mode_active_at_end(pattern: &str) -> bool {
    let mut chars = pattern.chars().peekable();
    let mut escaped = false;
    let mut in_class = false;
    let mut verbose = false;

    while let Some(ch) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '#' if verbose && !in_class => {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class && chars.next_if_eq(&'?').is_some() => {
                let mut flags = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == ')' || ch == ':' {
                        break;
                    }
                    if !matches!(ch, 'a' | 'i' | 'L' | 'm' | 's' | 'u' | 'x' | '-') {
                        flags.clear();
                        break;
                    }
                    flags.push(ch);
                    chars.next();
                }
                if chars.next_if_eq(&')').is_some() {
                    apply_verbose_flag(&mut verbose, &flags);
                }
            }
            _ => {}
        }
    }

    verbose
}

fn apply_verbose_flag(verbose: &mut bool, flags: &str) {
    let mut removing = false;
    for ch in flags.chars() {
        match ch {
            '-' => removing = true,
            'x' => *verbose = !removing,
            _ => {}
        }
    }
}

/// Returns the next valid UTF-8 boundary after `pos`, or one past the end.
fn next_utf8_boundary(text: &str, pos: usize) -> usize {
    text.get(pos..)
        .and_then(|text| text.chars().next())
        .map_or(text.len() + 1, |ch| pos + ch.len_utf8())
}

/// Python `re.escape(pattern)`.
/// Escapes special characters in a string so it can be used as a literal pattern in a regex.
/// According to Python 3.7+ behavior, escapes these characters: \ . ^ $ * + ? { } [ ] ( ) |
fn re_escape(args: &[Value]) -> Result<Value, Error> {
    if args.is_empty() {
        return Err(Error::new(
            ErrorKind::MissingArgument,
            "escape() requires a pattern string argument",
        ));
    }

    let pattern = args[0].to_string();
    let mut escaped = String::with_capacity(pattern.len() * 2);

    for ch in pattern.chars() {
        match ch {
            '\\' | '.' | '^' | '$' | '*' | '+' | '?' | '{' | '}' | '[' | ']' | '(' | ')' | '|' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => {
                escaped.push(ch);
            }
        }
    }

    Ok(Value::from(escaped))
}

/// Compile a pattern string with optional inline flags prefix.
fn compile_pattern(pattern: &str, flags: i64) -> Result<Box<Regex>, Error> {
    let full_pattern = if flags != 0 {
        format!("{}{}", flags_to_inline_prefix(flags), pattern)
    } else {
        pattern.to_string()
    };
    Ok(Box::new(Regex::new(&full_pattern).map_err(|e| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("Failed to compile regex: {e}"),
        )
    })?))
}

/// Extract either a compiled regex from arg[0] *or* compile arg[0], plus read `string` from arg[1].
/// If `flags` is non-zero and the pattern is a raw string (not pre-compiled), inline flags are
/// prepended to the pattern.
fn get_or_compile_regex_and_text_with_flags(
    args: &[Value],
    flags: i64,
) -> Result<(Box<Regex>, &str), Error> {
    if args.len() < 2 {
        return Err(Error::new(
            ErrorKind::MissingArgument,
            "Need at least pattern and string arguments",
        ));
    }

    let compiled = if let Some(object) = args[0].as_object() {
        if let Some(pattern) = object.downcast_ref::<Pattern>() {
            if flags != 0 {
                compile_pattern(pattern.compiled.as_str(), flags)?
            } else {
                Box::new(pattern.compiled.clone())
            }
        } else {
            compile_pattern(&args[0].to_string(), flags)?
        }
    } else {
        compile_pattern(&args[0].to_string(), flags)?
    };

    let text = args[1].to_string();
    Ok((compiled, Box::leak(text.into_boxed_str())))
}

/// Utility: turn a single match range into a quick list describing the match start/end/group0.
fn match_obj_to_list(re: &Regex, text: &str, start: usize, end: usize) -> Value {
    if let Ok(Some(caps)) = re.captures(&text[start..end]) {
        // We'll store (group0, group1, ...) as a list of strings or None
        let mut cap_vals = Vec::with_capacity(caps.len());
        for i in 0..caps.len() {
            cap_vals.push(Value::from(caps.get(i).map(|m| m.as_str()).unwrap_or("")));
        }
        Value::from(cap_vals)
    } else {
        // If for some reason capturing fails, just store the entire match
        Value::from(&text[start..end])
    }
}

type Span = (usize, usize);

#[derive(Debug, Clone)]
pub struct Capture {
    groups: Vec<(Value, Option<Span>)>,
    named_groups: IndexMap<String, usize>,
    input_string: String,
    pattern: String,
    pos: usize,
    endpos: usize,
    /// True when created by re.findall (groups already skip group 0).
    /// Only findall captures expose sequence semantics; match/search captures
    /// behave like Python Match objects and do not support index access.
    is_findall: bool,
}

impl Capture {
    pub fn new(
        groups: Vec<(Value, Option<Span>)>,
        named_groups: IndexMap<String, usize>,
        input_string: String,
        pattern: String,
    ) -> Self {
        let endpos = input_string.len();
        Self {
            groups,
            named_groups,
            input_string,
            pattern,
            pos: 0,
            endpos,
            is_findall: false,
        }
    }

    pub fn new_findall(
        groups: Vec<(Value, Option<Span>)>,
        named_groups: IndexMap<String, usize>,
        input_string: String,
        pattern: String,
    ) -> Self {
        let mut capture = Self::new(groups, named_groups, input_string, pattern);
        capture.is_findall = true;
        capture
    }

    /// Helper: parse the [group] argument, which could be an index or the name of a group
    fn get_group_idx_from_value(self: &std::sync::Arc<Self>, arg: &Value) -> Result<usize, Error> {
        if let Some(idx) = arg.as_usize() {
            Ok(idx)
        } else if let Some(group) = arg.as_str() {
            self.named_groups
                .get(group)
                .copied()
                .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "no such group"))
        } else {
            Err(Error::new(
                ErrorKind::InvalidArgument,
                "group argument must be an int or string",
            ))
        }
    }
}

impl Object for Capture {
    fn call_method(
        self: &std::sync::Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            // Match.expand(template)
            "expand" => {
                // https://docs.python.org/3/library/re.html#re.Match.expand
                todo!("'expand' is not yet implemented")
            }
            // Match.group([group1, ...])
            "group" => {
                if args.len() > 1 {
                    let mut groups: Vec<Value> = Vec::with_capacity(args.len());

                    for arg in args {
                        let idx = self.get_group_idx_from_value(arg)?;
                        groups.push(self.groups[idx].0.clone());
                    }

                    Ok(Value::from_tuple(groups))
                } else {
                    let idx = if args.is_empty() {
                        0
                    } else {
                        self.get_group_idx_from_value(&args[0])?
                    };

                    if idx < self.groups.len() {
                        Ok(self.groups[idx].0.clone())
                    } else {
                        Err(Error::new(ErrorKind::InvalidArgument, "no such group"))
                    }
                }
            }
            // Match.groups(default=None)
            "groups" => {
                let iter = ArgsIter::new(method, &[], args);
                let default = iter
                    .next_kwarg::<Option<Value>>("default")?
                    .unwrap_or(Value::NONE);
                let groups = Vec::from_iter(self.groups.iter().skip(1).map(|(group, _)| {
                    if group.is_none() {
                        default.clone()
                    } else {
                        group.clone()
                    }
                }));
                Ok(Value::from_tuple(groups))
            }
            // Match.groupdict(default=None)
            "groupdict" => {
                let iter = ArgsIter::new(method, &[], args);
                let default = iter
                    .next_kwarg::<Option<Value>>("default")?
                    .unwrap_or(Value::NONE);
                let named_groups =
                    ValueMap::from_iter(self.named_groups.iter().map(|(name, idx)| {
                        let group = &self.groups[*idx].0;
                        if group.is_none() {
                            (Value::from(name), default.clone())
                        } else {
                            (Value::from(name), group.clone())
                        }
                    }));
                Ok(Value::from(named_groups))
            }
            // Match.start([group])
            "start" => {
                let iter = ArgsIter::new(method, &["group"], args);
                let idx = if let Ok(arg) = iter.next_arg() {
                    self.get_group_idx_from_value(arg)?
                } else {
                    0
                };
                iter.finish()?;

                if idx < self.groups.len() {
                    if let Some((start, _)) = self.groups[idx].1 {
                        Ok(Value::from(start))
                    } else {
                        Ok(Value::from(-1))
                    }
                } else {
                    Err(Error::new(ErrorKind::InvalidArgument, "no such group"))
                }
            }
            // Match.end([group])
            "end" => {
                let iter = ArgsIter::new(method, &["group"], args);
                let idx = if let Ok(arg) = iter.next_arg() {
                    self.get_group_idx_from_value(arg)?
                } else {
                    0
                };
                iter.finish()?;

                if idx < self.groups.len() {
                    if let Some((_, end)) = self.groups[idx].1 {
                        Ok(Value::from(end))
                    } else {
                        Ok(Value::from(-1))
                    }
                } else {
                    Err(Error::new(ErrorKind::InvalidArgument, "no such group"))
                }
            }
            // Match.span([group])
            "span" => {
                let iter = ArgsIter::new(method, &["group"], args);
                let idx = if let Ok(arg) = iter.next_arg() {
                    self.get_group_idx_from_value(arg)?
                } else {
                    0
                };
                iter.finish()?;

                if idx < self.groups.len() {
                    if let Some((start, end)) = self.groups[idx].1 {
                        Ok(Value::from_tuple(vec![
                            Value::from(start),
                            Value::from(end),
                        ]))
                    } else {
                        Ok(Value::from_tuple(vec![Value::from(-1), Value::from(-1)]))
                    }
                } else {
                    Err(Error::new(ErrorKind::InvalidArgument, "no such group"))
                }
            }
            _ => Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("Method '{method}' not found!"),
            )),
        }
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        if self.is_findall {
            if let Some(idx) = key.as_usize() {
                return self.groups.get(idx).map(|(v, _)| v.clone());
            }
        }
        match key.as_str()? {
            "pos" => Some(Value::from(self.pos)),
            "endpos" => Some(Value::from(self.endpos)),
            "lastindex" => {
                let last = self
                    .groups
                    .iter()
                    .enumerate()
                    .skip(1)
                    .rev()
                    .find(|(_, (val, _))| !val.is_none());
                match last {
                    Some((idx, _)) => Some(Value::from(idx)),
                    None => Some(Value::from(())),
                }
            }
            "lastgroup" => {
                let last_idx = self
                    .groups
                    .iter()
                    .enumerate()
                    .skip(1)
                    .rev()
                    .find(|(_, (val, _))| !val.is_none())
                    .map(|(idx, _)| idx);
                match last_idx {
                    Some(idx) => {
                        let name = self
                            .named_groups
                            .iter()
                            .find(|(_, &gi)| gi == idx)
                            .map(|(name, _)| name.clone());
                        match name {
                            Some(n) => Some(Value::from(n)),
                            None => Some(Value::from(())),
                        }
                    }
                    None => Some(Value::from(())),
                }
            }
            "re" => {
                let compiled = Regex::new(&self.pattern).ok()?;
                Some(Value::from_object(Pattern::new(&self.pattern, compiled)))
            }
            "string" => Some(Value::from(self.input_string.clone())),
            _ => None,
        }
    }

    fn repr(self: &Arc<Self>) -> ObjectRepr {
        if self.is_findall {
            ObjectRepr::Seq
        } else {
            ObjectRepr::Map
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        if self.is_findall {
            let values: Vec<Value> = self.groups.iter().map(|(v, _)| v.clone()).collect();
            Enumerator::Values(values)
        } else {
            Enumerator::NonEnumerable
        }
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        if self.is_findall {
            Some(self.groups.len())
        } else {
            None
        }
    }

    fn is_true(self: &Arc<Self>) -> bool {
        !self.groups.is_empty()
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result
    where
        Self: Sized + 'static,
    {
        write!(f, "<re.Match object; ")?;
        if let Some((g, span)) = self.groups.first() {
            if let Some((start, end)) = span {
                write!(f, "span = ({start}, {end}), ")?;
            }
            // TODO: escape quotes in g
            write!(f, "match = '{g}'")?;
        }
        write!(f, ">")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_re_sub() {
        let result = re_sub(&[
            Value::from("(A)".to_string()),
            Value::from("_\\1_".to_string()),
            Value::from("ABAB $1".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "_A_B_A_B $1");

        let result = re_sub(&[
            Value::from("(A)".to_string()),
            Value::from("_\\1_".to_string()),
            Value::from("ABAB $1".to_string()),
            Value::from(1),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "_A_BAB $1");

        let result = re_sub(&[
            Value::from("x*".to_string()),
            Value::from("-".to_string()),
            Value::from("abxd".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "-a-b--d-");

        let result = re_sub(&[
            Value::from("x*".to_string()),
            Value::from("-".to_string()),
            Value::from("abxd".to_string()),
            Value::from(2),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "-a-bxd");

        let result = re_sub(&[
            Value::from("".to_string()),
            Value::from("-".to_string()),
            Value::from("é".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "-é-");

        let result = re_sub(&[
            Value::from(".*?".to_string()),
            Value::from("-".to_string()),
            Value::from("ab".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "-----");

        let result = re_sub(&[
            Value::from("(.*?)".to_string()),
            Value::from("<\\1>".to_string()),
            Value::from("ab".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "<><a><><b><>");

        let result = re_sub(&[
            Value::from("|^a".to_string()),
            Value::from("-".to_string()),
            Value::from("ba".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "-b-a-");

        let result = re_sub(&[
            Value::from("|a+".to_string()),
            Value::from("-".to_string()),
            Value::from("aa".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "---");

        let result = re_sub(&[
            Value::from("(?x)| a # comment".to_string()),
            Value::from("-".to_string()),
            Value::from("a".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "---");

        let result = re_sub(&[
            Value::from("(?x)| a # (?-x)".to_string()),
            Value::from("-".to_string()),
            Value::from("a".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "---");

        let result = re_sub(&[
            Value::from("|(?x:a)".to_string()),
            Value::from("-".to_string()),
            Value::from("a".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "---");

        let result = re_sub(&[
            Value::from("(?=x)|x".to_string()),
            Value::from("-".to_string()),
            Value::from("x".to_string()),
        ])
        .unwrap();
        assert_eq!(result.to_string(), "--");
    }

    #[test]
    fn test_re_match() {
        let result = re_match(&[
            Value::from(".*".to_string()),
            Value::from("xyz".to_string()),
        ])
        .unwrap();
        assert!(result.is_true());
        assert_eq!(
            result.to_string(),
            "<re.Match object; span = (0, 3), match = 'xyz'>"
        );

        let result = re_match(&[
            Value::from("\\d{10}".to_string()),
            Value::from("1234567890".to_string()),
        ])
        .unwrap();
        assert!(result.is_true());
        assert_eq!(
            result.to_string(),
            "<re.Match object; span = (0, 10), match = '1234567890'>"
        );

        let result = re_match(&[
            Value::from("\\d{10}".to_string()),
            Value::from("xyz".to_string()),
        ])
        .unwrap();
        assert!(!result.is_true());
        assert_eq!(result.to_string(), "None");
    }

    #[test]
    fn test_re_search() {
        let result = re_search(&[
            Value::from("world".to_string()),
            Value::from("hello, world".to_string()),
        ])
        .unwrap();
        assert!(result.is_true());
        assert_eq!(
            result.to_string(),
            "<re.Match object; span = (7, 12), match = 'world'>"
        );

        let result = re_search(&[
            Value::from("hello".to_string()),
            Value::from("world".to_string()),
        ])
        .unwrap();
        assert!(!result.is_true());
        assert_eq!(result.to_string(), "None");
    }

    #[test]
    fn test_re_glob_search() {
        let result = re_search(&[
            Value::from(".*".to_string()),
            Value::from("xyz".to_string()),
        ])
        .unwrap();
        assert!(result.is_true());

        let compiled_pattern = re_compile(&[Value::from(".*".to_string())]).unwrap();
        let result = re_search(&[compiled_pattern, Value::from("xyz".to_string())]).unwrap();
        assert!(result.is_true());
    }

    #[test]
    fn test_re_escape() {
        // Test basic special character escaping
        let result = re_escape(&[Value::from("hello.world")]).unwrap();
        assert_eq!(result.to_string(), r"hello\.world");

        // Test multiple special characters
        let result = re_escape(&[Value::from("$100+")]).unwrap();
        assert_eq!(result.to_string(), r"\$100\+");

        // Test all metacharacters
        let result = re_escape(&[Value::from(r"\^$.*+?{}[]|()")]).unwrap();
        assert_eq!(result.to_string(), r"\\\^\$\.\*\+\?\{\}\[\]\|\(\)");

        // Test string with no special characters
        let result = re_escape(&[Value::from("hello")]).unwrap();
        assert_eq!(result.to_string(), "hello");

        // Test empty string
        let result = re_escape(&[Value::from("")]).unwrap();
        assert_eq!(result.to_string(), "");

        // Test typical suffix pattern (the use case from the issue)
        let result = re_escape(&[Value::from("_usd$")]).unwrap();
        assert_eq!(result.to_string(), r"_usd\$");
    }

    #[test]
    fn test_re_escape_missing_argument() {
        let result = re_escape(&[]);
        assert!(result.is_err());
    }

    // Regression test: Capture objects returned by re.findall with 3+ groups must be
    // sliceable sequences (Python returns N-tuples, not mappings).
    #[test]
    fn test_re_findall_capture_is_sliceable_seq() {
        use minijinja::Environment;

        let mut env = Environment::new();
        env.add_global("re", Value::from(create_re_namespace()));

        // Pattern with 3 capture groups → findall returns Capture objects.
        // `match[1:]|join(",")` is the exact pattern used by dbt-project-evaluator.
        let result = env
            .render_str(
                r#"{% set matches = re.findall('(a)(b)(c)', 'abc') %}{{ matches[0][1:]|join(",") }}"#,
                (),
                &[],
            )
            .expect("slicing a multi-group findall result should not fail");
        assert_eq!(result, "b,c");

        // Index access: match[0] → first capture group (group 1 in regex terms)
        let result = env
            .render_str(
                r#"{% set matches = re.findall('(\w+):(\w+):(\w+)', 'foo:bar:baz') %}{{ matches[0][0] }},{{ matches[0][1] }},{{ matches[0][2] }}"#,
                (),
                &[],
            )
            .expect("integer index access should work");
        assert_eq!(result, "foo,bar,baz");

        // Length via `|length` filter
        let result = env
            .render_str(
                r#"{% set matches = re.findall('(x)(y)(z)', 'xyz') %}{{ matches[0]|length }}"#,
                (),
                &[],
            )
            .expect("length filter should work on Capture");
        assert_eq!(result, "3");
    }

    // Match/search captures must NOT expose sequence semantics: slicing should fail
    // because Python's Match object is not a sequence.
    #[test]
    fn test_re_match_capture_is_not_sliceable() {
        use minijinja::Environment;

        let mut env = Environment::new();
        env.add_global("re", Value::from(create_re_namespace()));

        // re.match with groups returns a Match object — slicing must be an error
        let result = env.render_str(
            r#"{% set m = re.match('(a)(b)(c)', 'abc') %}{{ m[1:] }}"#,
            (),
            &[],
        );
        assert!(
            result.is_err(),
            "slicing a match/search Capture should fail, got: {result:?}"
        );

        // But Match object methods still work normally
        let result = env
            .render_str(
                r#"{% set m = re.match('(a)(b)(c)', 'abc') %}{{ m.group(1) }},{{ m.group(2) }}"#,
                (),
                &[],
            )
            .expect("match.group() should still work");
        assert_eq!(result, "a,b");
    }

    fn run_re_template(template: &str) -> String {
        use minijinja::Environment;
        let mut env = Environment::new();
        env.add_global("re", Value::from(create_re_namespace()));
        env.render_str(template, (), &[]).unwrap()
    }

    #[test]
    fn test_re_findall_flags_kwarg() {
        // Without IGNORECASE: only lowercase matches
        let result = run_re_template(r#"{{ re.findall('[a-z]+', 'Hello World')|length }}"#);
        assert_eq!(result, "2"); // "ello", "orld"

        // With IGNORECASE via kwarg: matches full words
        let result = run_re_template(
            r#"{{ re.findall('[a-z]+', 'Hello World', flags=re.IGNORECASE)|join(',') }}"#,
        );
        assert_eq!(result, "Hello,World");
    }

    #[test]
    fn test_re_findall_dotall_kwarg() {
        // The actual bug from #15508: DOTALL via kwarg for multi-line matching
        let result = run_re_template(
            r#"{{ re.findall('begin(.*)end', 'begin\nfoo\nend', flags=re.DOTALL)|length }}"#,
        );
        assert_eq!(result, "1");
    }

    #[test]
    fn test_re_sub_count_kwarg() {
        let result = run_re_template(r#"{{ re.sub('a', 'b', 'aaa', count=1) }}"#);
        assert_eq!(result, "baa");
    }

    #[test]
    fn test_re_split_maxsplit_kwarg() {
        let result = run_re_template(r#"{{ re.split(':', 'a:b:c', maxsplit=1)|join(',') }}"#);
        assert_eq!(result, "a,b:c");
    }

    #[test]
    fn test_re_compile_flags_kwarg() {
        let result = run_re_template(
            r#"{% set p = re.compile('[a-z]+', flags=re.IGNORECASE) %}{{ p.findall('Hello')|join(',') }}"#,
        );
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_re_search_flags_kwarg() {
        let result = run_re_template(
            r#"{% set m = re.search('[a-z]+', 'HELLO', flags=re.IGNORECASE) %}{{ m.group(0) }}"#,
        );
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_re_match_flags_kwarg() {
        let result = run_re_template(
            r#"{% set m = re.match('[a-z]+', 'HELLO', flags=re.IGNORECASE) %}{{ m.group(0) }}"#,
        );
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_pattern_split_maxsplit_kwarg() {
        let result = run_re_template(
            r#"{% set p = re.compile(':') %}{{ p.split('a:b:c', maxsplit=1)|join(',') }}"#,
        );
        assert_eq!(result, "a,b:c");
    }

    #[test]
    fn test_pattern_sub_count_kwarg() {
        let result =
            run_re_template(r#"{% set p = re.compile('a') %}{{ p.sub('b', 'aaa', count=1) }}"#);
        assert_eq!(result, "baa");
    }
}
