// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! A parser for [`Duration`] following <https://pkg.go.dev/time#ParseDuration>
//!
//!

use std::{error::Error as StdError, num::IntErrorKind, sync::LazyLock, time::Duration};

use adbc_core::error::{Error, Result, Status};
use regex::Regex;

// The regular expression used to parse durations.
static RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<int>[0-9]*)\.?(?P<frac>[0-9]*)(?P<unit>ns|us|µs|μs|ms|s|m|h)")
        .expect("valid regex")
});

fn invalid_arg(err: impl Into<String>) -> Error {
    Error::with_message_and_status(err.into(), Status::InvalidArguments)
}

fn arg_err(err: impl StdError) -> Error {
    invalid_arg(err.to_string())
}

fn overflow() -> Error {
    invalid_arg("duration overflow")
}

fn bad_input<T>() -> Result<T> {
    Err(invalid_arg(
        "invalid duration (valid durations are a sequence of decimal numbers, each with optional fraction and a unit suffix, such as 300ms, 1.5h, 2h45m, valid time units are ns, us, ms, s, m, h)",
    ))
}

/// Parse the given string to a [`Duration`], returning an eror when parsing
/// fails.
///
/// Following the logic of <https://pkg.go.dev/time#ParseDuration>, except this
/// implementation does not support negative values i.e. it ignores sign
/// symbols, because [`Duration`] does not support negative values, and the Go
/// Snowflake driver uses absolute values for these durations.
pub fn parse_duration(input: &str) -> Result<Duration> {
    // Drop sign symbols.
    let input = input.replace(['+', '-'], "");

    // Special case for zero.
    if input == "0" {
        return Ok(Duration::ZERO);
    }

    // Other bad input when there is no match.
    if RE.find(&input).is_none() {
        return bad_input();
    }

    // Aggregate durations.
    RE.captures_iter(&input)
        .map(|caps| caps.extract())
        .try_fold(Duration::ZERO, |mut duration, (_, [int, frac, unit])| {
            // Int and frac can't both be empty.
            if int.is_empty() && frac.is_empty() {
                return bad_input();
            }

            // Parse int part.
            let int = int
                .parse::<u64>()
                .or_else(|err| match err.kind() {
                    // Int can be empty to support .<frac> notation.
                    &IntErrorKind::Empty => Ok(0),
                    _ => Err(err),
                })
                .map_err(arg_err)?;

            // Parse frac part.
            let frac = format!("0.{frac}").parse::<f64>().map_err(arg_err)?;

            // Add this part to the duration.
            duration += match unit {
                "h" => {
                    Duration::from_secs(3600u64.checked_mul(int).ok_or_else(overflow)?)
                        + Duration::try_from_secs_f64(3600f64 * frac).map_err(arg_err)?
                }
                "m" => {
                    Duration::from_secs(60u64.checked_mul(int).ok_or_else(overflow)?)
                        + Duration::try_from_secs_f64(60f64 * frac).map_err(arg_err)?
                }
                "s" => {
                    Duration::from_secs(int) + Duration::try_from_secs_f64(frac).map_err(arg_err)?
                }
                "ms" => {
                    Duration::from_millis(int)
                        + Duration::try_from_secs_f64(frac / 1e3).map_err(arg_err)?
                }
                "us" | "µs" | "μs" => {
                    Duration::from_micros(int)
                        + Duration::try_from_secs_f64(frac / 1e6).map_err(arg_err)?
                }
                "ns" => {
                    if frac != 0f64 {
                        return Err(invalid_arg(
                            "unexpected fractional part for duration with ns unit",
                        ));
                    }
                    Duration::from_nanos(int)
                }
                _ => unreachable!("<unit> matching group prevents this branch"),
            };

            Ok(duration)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::cognitive_complexity)]
    #[test]
    fn parse_duration_test() {
        assert_eq!(parse_duration("0"), Ok(Duration::from_secs(0)));
        assert_eq!(parse_duration("0000.0000s"), Ok(Duration::from_secs(0)));
        assert_eq!(parse_duration("5s"), Ok(Duration::from_secs(5)));
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("1478s"), Ok(Duration::from_secs(1478)));
        assert_eq!(parse_duration("-5s"), Ok(Duration::from_secs(5)));
        assert_eq!(parse_duration("+5s"), Ok(Duration::from_secs(5)));
        assert_eq!(parse_duration("-0"), Ok(Duration::from_secs(0)));
        assert_eq!(parse_duration("+0"), Ok(Duration::from_secs(0)));
        assert_eq!(parse_duration("5.0s"), Ok(Duration::from_secs(5)));
        assert_eq!(
            parse_duration("5.6s"),
            Ok(Duration::from_secs(5) + Duration::from_millis(600))
        );
        assert_eq!(parse_duration("5.s"), Ok(Duration::from_secs(5)));
        assert_eq!(parse_duration(".5s"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("1.0s"), Ok(Duration::from_secs(1)));
        assert_eq!(parse_duration("1.00s"), Ok(Duration::from_secs(1)));
        assert_eq!(
            parse_duration("1.004s"),
            Ok(Duration::from_secs(1) + Duration::from_millis(4))
        );
        assert_eq!(
            parse_duration("1.0040s"),
            Ok(Duration::from_secs(1) + Duration::from_millis(4))
        );
        assert_eq!(
            parse_duration("100.00100s"),
            Ok(Duration::from_secs(100) + Duration::from_millis(1))
        );
        assert_eq!(parse_duration("10ns"), Ok(Duration::from_nanos(10)));
        assert_eq!(parse_duration("11us"), Ok(Duration::from_micros(11)));
        assert_eq!(parse_duration("12µs"), Ok(Duration::from_micros(12)));
        assert_eq!(parse_duration("12μs"), Ok(Duration::from_micros(12)));
        assert_eq!(parse_duration("13ms"), Ok(Duration::from_millis(13)));
        assert_eq!(parse_duration("14s"), Ok(Duration::from_secs(14)));
        assert_eq!(parse_duration("15m"), Ok(Duration::from_secs(60 * 15)));
        assert_eq!(parse_duration("16h"), Ok(Duration::from_secs(60 * 60 * 16)));
        assert_eq!(
            parse_duration("3h30m"),
            Ok(Duration::from_secs(60 * 60 * 3) + Duration::from_secs(60 * 30))
        );
        assert_eq!(
            parse_duration("10.5s4m"),
            Ok(Duration::from_secs(10) + Duration::from_millis(500) + Duration::from_secs(60 * 4))
        );
        assert_eq!(
            parse_duration("-2m3.4s"),
            Ok(Duration::from_secs(60 * 2) + Duration::from_secs(3) + Duration::from_millis(400))
        );
        assert_eq!(
            parse_duration("1h2m3s4ms5us6ns"),
            Ok(Duration::from_secs(60 * 60)
                + Duration::from_secs(60 * 2)
                + Duration::from_secs(3)
                + Duration::from_millis(4)
                + Duration::from_micros(5)
                + Duration::from_nanos(6))
        );
        assert_eq!(
            parse_duration("39h9m14.425s"),
            Ok(Duration::from_secs(60 * 60 * 39)
                + Duration::from_secs(60 * 9)
                + Duration::from_secs(14)
                + Duration::from_millis(425))
        );
        assert_eq!(
            parse_duration("52763797000ns"),
            Ok(Duration::from_nanos(52763797000))
        );
        assert_eq!(
            parse_duration("0.3333333333333333333h"),
            Ok(Duration::from_secs(60 * 20))
        );
        assert_eq!(
            parse_duration("9007199254740993ns"),
            Ok(Duration::from_nanos(9007199254740993))
        );
        assert_eq!(
            parse_duration("9223372036854775807ns"),
            Ok(Duration::from_nanos(9223372036854775807))
        );
        assert_eq!(
            parse_duration("9223372036854775.807us"),
            Ok(Duration::from_micros(9223372036854775) + Duration::from_nanos(807))
        );
        assert_eq!(
            parse_duration("9223372036s854ms775us807ns"),
            Ok(Duration::from_secs(9223372036)
                + Duration::from_millis(854)
                + Duration::from_micros(775)
                + Duration::from_nanos(807))
        );
        assert_eq!(
            parse_duration("-9223372036854775808ns"),
            Ok(Duration::from_nanos(9223372036854775808))
        );
        assert_eq!(
            parse_duration("-9223372036854775.808us"),
            Ok(Duration::from_micros(9223372036854775) + Duration::from_nanos(808))
        );
        assert_eq!(
            parse_duration("-9223372036s854ms775us808ns"),
            Ok(Duration::from_secs(9223372036)
                + Duration::from_millis(854)
                + Duration::from_micros(775)
                + Duration::from_nanos(808))
        );
        assert_eq!(
            parse_duration("-9223372036854775808ns"),
            Ok(Duration::from_nanos(9223372036854775808))
        );
        assert_eq!(
            parse_duration("-2562047h47m16.854775808s"),
            Ok(Duration::from_secs(60 * 60 * 2562047)
                + Duration::from_secs(60 * 47)
                + Duration::from_secs(16)
                + Duration::from_nanos(854775808))
        );
        assert_eq!(
            parse_duration("0.100000000000000000000h"),
            Ok(Duration::from_secs(60 * 6))
        );
        assert_eq!(
            parse_duration("0.830103483285477580700h"),
            Ok(Duration::from_secs(60 * 49)
                + Duration::from_secs(48)
                + Duration::from_nanos(372539828))
        );
        let bad_input = Err(invalid_arg(
            "invalid duration (valid durations are a sequence of decimal numbers, each with optional fraction and a unit suffix, such as 300ms, 1.5h, 2h45m, valid time units are ns, us, ms, s, m, h)",
        ));
        assert_eq!(parse_duration(""), bad_input);
        assert_eq!(parse_duration("3"), bad_input);
        assert_eq!(parse_duration("-"), bad_input);
        assert_eq!(parse_duration("s"), bad_input);
        assert_eq!(parse_duration("."), bad_input);
        assert_eq!(parse_duration("-."), bad_input);
        assert_eq!(parse_duration(".s"), bad_input);
        assert_eq!(parse_duration("+.s"), bad_input);
        assert_eq!(parse_duration("1d"), bad_input);
        assert_eq!(
            parse_duration("1.1ns"),
            Err(invalid_arg(
                "unexpected fractional part for duration with ns unit"
            ))
        );
        assert_eq!(parse_duration("9999999999999999h"), Err(overflow()));
    }
}
