use std::collections::HashMap;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use minijinja::{Error, ErrorKind};
use regex::{Captures, Regex};

// See: https://docs.rs/regex/latest/regex/struct.Regex.html#method.replace_all
fn try_replace_all<E>(
    re: &Regex,
    haystack: &str,
    replacement: impl Fn(&Captures) -> Result<String, E>,
) -> Result<String, E> {
    let mut new = String::with_capacity(haystack.len());
    let mut last_match = 0;
    for caps in re.captures_iter(haystack) {
        let m = caps.get(0).unwrap();
        new.push_str(&haystack[last_match..m.start()]);
        new.push_str(&replacement(&caps)?);
        last_match = m.end();
    }
    new.push_str(&haystack[last_match..]);
    Ok(new)
}

struct TimeRE {
    // TODO: Add the LocaleTime object
    directives: HashMap<&'static str, String>,
}

impl TimeRE {
    /// Using locale, generate a new mapping of directives to regex patterns for strptime parsing.
    /// Reference: https://github.com/python/cpython/blob/3514ba2175764e4d746e6862e2fcfc06b5fcd6c2/Lib/_strptime.py#L343
    pub fn new() -> Self {
        // TODO: Not all of the format codes are covered here.
        // The remaining ones fall into the following cases:
        //  - Use the user's locale to build a regex pattern
        //  - Are aliases for specific combinations of other patterns
        //  - Use the %O modifier (see https://man7.org/linux/man-pages/man3/strptime.3.html)
        // The %O modifier seems to be supported but _undocumented_ in Python's strptime, but
        // it's in the source code.
        let directives = HashMap::from([
            (
                "d",
                r#"(?P<d>3[0-1]|[1-2]\d|0[1-9]|[1-9]| [1-9])"#.to_owned(),
            ),
            ("f", r#"(?P<f>[0-9]{1,6})"#.to_owned()),
            ("H", r#"(?P<H>2[0-3]|[0-1]\d|\d| \d)"#.to_owned()),
            ("k", r#"(?P<H>2[0-3]|[0-1]\d|\d| \d)"#.to_owned()),
            ("I", r#"(?P<I>1[0-2]|0[1-9]|[1-9]| [1-9])"#.to_owned()),
            ("l", r#"(?P<I>1[0-2]|0[1-9]|[1-9]| [1-9])"#.to_owned()),
            ("G", r#"(?P<G>\d\d\d\d)"#.to_owned()),
            (
                "j",
                r#"(?P<j>36[0-6]|3[0-5]\d|[1-2]\d\d|0[1-9]\d|00[1-9]|[1-9]\d|0[1-9]|[1-9])"#
                    .to_owned(),
            ),
            ("m", r#"(?P<m>1[0-2]|0[1-9]|[1-9])"#.to_owned()),
            ("M", r#"(?P<M>[0-5]\d|\d)"#.to_owned()),
            ("S", r#"(?P<S>6[0-1]|[0-5]\d|\d)"#.to_owned()),
            ("U", r#"(?P<U>5[0-3]|[0-4]\d|\d)"#.to_owned()),
            ("w", r#"(?P<w>[0-6])"#.to_owned()),
            ("u", r#"(?P<u>[1-7])"#.to_owned()),
            ("V", r#"(?P<V>5[0-3]|0[1-9]|[1-4]\d|\d)"#.to_owned()),
            ("y", r#"(?P<y>\d\d)"#.to_owned()),
            ("Y", r#"(?P<Y>\d\d\d\d)"#.to_owned()),
            (
                "z",
                r#"(?P<z>([+-]\d\d:?[0-5]\d(:?[0-5]\d(\.\d{1,6})?)?)|(?-i:Z))?"#.to_owned(),
            ),
            (
                ":z",
                r#"(?P<colon_z>([+-]\d\d:[0-5]\d(:[0-5]\d(\.\d{1,6})?)?)|(?-i:Z))?"#.to_owned(),
            ),
            ("%", r#"%"#.to_owned()),
        ]);
        TimeRE { directives }
    }

    /// Process a format string into a compiled regex.
    /// Reference: https://github.com/python/cpython/blob/3514ba2175764e4d746e6862e2fcfc06b5fcd6c2/Lib/_strptime.py#L448
    pub fn regex_from_fmt(&self, fmt: &str) -> Result<Regex, Error> {
        // Preprocess `fmt` to ignore regex characters and handle whitespace
        let sub_patterns = [
            (r#"([\\.^$*+?\(\){}\[\]|])"#, r#"\\\1"#), // Escape characters used in regex syntax
            (r#"\s+"#, r#"\s+"#),                      // Match any amount of whitespace
            (r#"'"#, r#"['\u02bc]"#),                  // Special case for br_FR
        ];
        let mut fmt = fmt.to_string();
        for (pattern, rep) in sub_patterns {
            let compiled = Regex::new(pattern).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    format!("Failed to compile regex: {e}"),
                )
            })?;
            fmt = compiled.replace_all(&fmt, rep).to_string();
        }

        // Replace all instances of directives with their regex patterns
        let directive_regex = Regex::new(r#"%[-_0^#]*[0-9]*([OE]?[:\\]?.?)"#).map_err(|e| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("Failed to compile regex: {e}"),
            )
        })?;
        let replacement = |caps: &Captures| -> Result<String, Error> {
            let directive = &caps.get(0).unwrap().as_str()[1..];
            self.directives.get(directive).map_or(
                Err(Error::new(
                    ErrorKind::InvalidArgument,
                    format!("Invalid directive %{directive}"),
                )),
                |pattern| Ok(pattern.clone()),
            )
        };

        let pattern = try_replace_all(&directive_regex, &fmt, replacement).map_err(|e| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("Invalid directive: {e}"),
            )
        })?;

        // Anchor with start and end to ensure a full match
        // TODO: no versions of Python compatible with dbt Core accept %:z yet.
        // Once we allow %:z, we need to change this to match only from the start anchor.
        Regex::new(&format!("^{pattern}$")).map_err(|e| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("Failed to compile regex: {e}"),
            )
        })
    }
}

#[derive(Default, Debug)]
struct PartialDateTime {
    pub iso_year: Option<i32>,
    pub year: Option<i32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
    pub second: Option<u32>,
    pub fraction: Option<u32>,
    // TODO: add these fields once locale is taken into account
    // pub tz: Option<u32>,
    // pub gmtoff: Option<u32>,
    // pub gmtoff_fraction: Option<u32>,
    pub iso_week: Option<u32>,
    pub week_of_year: Option<u32>,
    pub week_of_year_start: Option<u32>,
    pub weekday: Option<u32>,
    pub julian: Option<u32>,
}

/// Reference: https://github.com/python/cpython/blob/3514ba2175764e4d746e6862e2fcfc06b5fcd6c2/Lib/_strptime.py#L518
pub fn strptime(input: &str, fmt: &str) -> Result<NaiveDateTime, String> {
    let format_regex = TimeRE::new()
        .regex_from_fmt(fmt)
        .map_err(|e| e.to_string())?;
    let caps = format_regex
        .captures(input)
        .ok_or_else(|| format!("Time data {input} does not match format {fmt}"))?;

    let parse_u32 =
        |s: &str| -> Result<u32, String> { s.parse::<u32>().map_err(|e| e.to_string()) };
    let parse_i32 =
        |s: &str| -> Result<i32, String> { s.parse::<i32>().map_err(|e| e.to_string()) };

    let mut date_time = PartialDateTime::default();

    for group_key in format_regex.capture_names().flatten() {
        if let Some(group_match) = caps.name(group_key) {
            let group = group_match.as_str();
            match group_key {
                "y" => {
                    let mut year = parse_i32(group)?;
                    if let Some(century_match) = caps.name("C") {
                        let century = parse_i32(century_match.as_str())?;
                        year += century * 100;
                    } else {
                        // See: https://github.com/python/cpython/blob/3514ba2175764e4d746e6862e2fcfc06b5fcd6c2/Lib/_strptime.py#L605
                        // Open Group specification for strptime() states that a %y
                        // value in the range of [00, 68] is in the century 2000, while
                        // [69,99] is in the century 1900
                        if year <= 68 {
                            year += 2000;
                        } else {
                            year += 1900;
                        }
                    }
                    date_time.year = Some(year);
                }
                "Y" => {
                    date_time.year = Some(parse_i32(group)?);
                }
                "G" => {
                    date_time.iso_year = Some(parse_i32(group)?);
                }
                "m" => {
                    date_time.month = Some(parse_u32(group)?);
                }
                // TODO: Implement %B and %b (month as locale's full/abbreviated name)
                "d" => {
                    date_time.day = Some(parse_u32(group)?);
                }
                "H" => {
                    date_time.hour = Some(parse_u32(group)?);
                }
                // TODO: Implement %I (12-hour clock hour)
                "M" => {
                    date_time.minute = Some(parse_u32(group)?);
                }
                "S" => {
                    date_time.second = Some(parse_u32(group)?);
                }
                "f" => {
                    // Second fraction is stored as number of microseconds, so we need to
                    // left-align pad with zeroes up to 6 digits.
                    let fraction = format!("{:0<6}", group);
                    date_time.fraction = Some(parse_u32(&fraction)?);
                }
                // TODO: Implement %A and %a (weekday as locale's full/abbreviated name)
                "w" => {
                    let weekday = parse_u32(group)?;
                    if weekday == 0 {
                        date_time.weekday = Some(6);
                    } else {
                        date_time.weekday = Some(weekday - 1);
                    }
                }
                "u" => {
                    let weekday = parse_u32(group)?;
                    date_time.weekday = Some(weekday - 1);
                }
                "j" => {
                    date_time.julian = Some(parse_u32(group)?);
                }
                "U" => {
                    date_time.week_of_year = Some(parse_u32(group)?);
                    date_time.week_of_year_start = Some(6);
                }
                "W" => {
                    date_time.week_of_year = Some(parse_u32(group)?);
                    date_time.week_of_year_start = Some(0);
                }
                "V" => {
                    date_time.iso_week = Some(parse_u32(group)?);
                }
                // TODO: Implement the %z, %:z, and %Z cases (UTC offset)
                _ => {
                    return Err(format!("unknown key {group_key}"));
                }
            }
        }
    }

    // Check for incompatible directives
    if date_time.iso_year.is_some() {
        if date_time.julian.is_some() {
            return Err("Day of the year directive '%j' is not \
                        compatible with ISO year directive '%G'. \
                        Use '%Y' instead."
                .to_string());
        } else if date_time.iso_week.is_none() || date_time.weekday.is_none() {
            return Err("ISO year directive '%G' must be used with \
                        the ISO week directive '%V' and a weekday \
                        directive ('%A', '%a', '%w', or '%u')."
                .to_string());
        }
    } else if date_time.iso_week.is_some() {
        if date_time.year.is_none() || date_time.weekday.is_none() {
            return Err("ISO week directive '%V' must be used with \
                        the ISO year directive '%G' and a weekday \
                        directive ('%A', '%a', '%w', or '%u')."
                .to_string());
        } else {
            return Err("ISO week directive '%V' is incompatible with \
                        the year directive '%Y'. Use the ISO year '%G' \
                        instead."
                .to_string());
        }
    }

    // TODO: This does not appear to be implemented in versions of Python supported by Core.
    // // Override the default year for February 29 to 1904.
    // if date_time.year.is_none() && date_time.month == Some(2) && date_time.day == Some(29) {
    //     date_time.year = Some(1904);
    // }

    // TODO: Julian and weekday calculations

    // TODO: Allow ISO directives to affect these
    let year = date_time.year.unwrap_or(1900);
    let month = date_time.month.unwrap_or(1);
    let day = date_time.day.unwrap_or(1);
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| format!("Could not create a NaiveDate from ymd {year} {month} {day}"))?;

    let hour = date_time.hour.unwrap_or(0);
    let minute = date_time.minute.unwrap_or(0);
    let second = date_time.second.unwrap_or(0);
    let micro = date_time.fraction.unwrap_or(0);
    let time = NaiveTime::from_hms_micro_opt(hour, minute, second, micro).ok_or_else(|| {
        format!("Could not create a NaiveTime from hms {hour}:{minute}:{second}.{micro}")
    })?;

    Ok(NaiveDateTime::new(date, time))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regex_from_fmt() {
        let time_re = TimeRE::new();

        for (fmt, successes, failures) in [
            (
                "%d",
                vec!["04", "13", "25", "31", " 9"],
                vec!["0", "500", "32", "00", "0003"],
            ),
            (
                "%f",
                vec!["0", "00", "112", "1024", "15451", "472999"],
                vec!["2847321", "0000000"],
            ),
            (
                "%H",
                vec!["00", "01", "09", "10", "23", " 9"],
                vec!["24", "99", "-1", "003"],
            ),
            (
                "%k",
                vec!["00", "01", "09", "10", "23", " 9"],
                vec!["24", "99", "-1", "003"],
            ),
            (
                "%I",
                vec!["01", "02", "09", "10", "12", " 1", " 9"],
                vec!["00", "13", "99", "-1"],
            ),
            (
                "%l",
                vec!["01", "02", "09", "10", "12", " 1", " 9"],
                vec!["00", "13", "99", "-1"],
            ),
            (
                "%j",
                vec!["001", "009", "010", "099", "100", "365", "366"],
                vec!["000", "367", "999", "-1"],
            ),
            (
                "%m",
                vec!["01", "02", "09", "10", "12"],
                vec!["00", "13", "99", "-1"],
            ),
            (
                "%M",
                vec!["00", "01", "09", "10", "59"],
                vec!["60", "99", "-1", "100"],
            ),
            (
                "%S",
                vec!["00", "01", "09", "10", "59"],
                vec!["62", "99", "-1", "100"],
            ),
            (
                "%U",
                vec!["00", "01", "09", "10", "53"],
                vec!["54", "99", "-1", "100"],
            ),
            (
                "%w",
                vec!["0", "1", "2", "3", "4", "5", "6"],
                vec!["7", "9", "-1", "00"],
            ),
            ("%y", vec!["00", "01", "69", "99"], vec!["0", "100", "-1"]),
            (
                "%Y",
                vec!["0001", "1970", "2024", "9999"],
                vec!["000", "10000", "-001"],
            ),
            (
                "%G%u%V",
                vec!["0001101", "1970202", "2024310", "9998752"],
                vec!["0000663", "1000999", "1000555"],
            ),
        ] {
            let compiled = time_re
                .regex_from_fmt(fmt)
                .expect("Expected regex to compile successfully.");
            for datestr in successes {
                let captures = compiled.captures(datestr);
                assert!(captures.is_some());
            }
            for datestr in failures {
                let captures = compiled.captures(datestr);
                assert!(captures.is_none());
            }
        }
    }

    #[test]
    fn test_strptime_from_fmt() {
        let date_str = "05/22";
        let fmt = "%m/%y";
        let expected = "2022-05-01T00:00:00.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );

        let date_str = "01/05";
        let fmt = "%m/%d";
        let expected = "1900-01-05T00:00:00.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );

        let date_str = "05";
        let fmt = "%d";
        let expected = "1900-01-05T00:00:00.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );

        let date_str = "05";
        let fmt = "%m";
        let expected = "1900-05-01T00:00:00.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );

        let date_str = "valid 10";
        let fmt = "valid %m";
        let expected = "1900-10-01T00:00:00.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );

        let date_str = "20";
        let fmt = "%y";
        let expected = "2020-01-01T00:00:00.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );

        let date_str = "05:20";
        let fmt = "%H:%M";
        let expected = "1900-01-01T05:20:00.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );

        let date_str = "05:20";
        let fmt = "%S:%M";
        let expected = "1900-01-01T00:20:05.000000+00:00";
        assert_eq!(
            strptime(date_str, fmt).unwrap_or_else(|_| panic!(
                "strptime should succed for input {date_str} and format {fmt}"
            )),
            NaiveDateTime::parse_from_str(expected, "%+").unwrap()
        );
    }
}
