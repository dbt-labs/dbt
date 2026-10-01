#![cfg(all(feature = "datetime", feature = "timezone"))]

use minijinja::context;
use similar_asserts::assert_eq;
use time::format_description::well_known::Iso8601;

#[test]
fn test_datetimeformat() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "Europe/Vienna");
    env.add_global("DATETIME_FORMAT", "[hour]:[minute]");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("1687624642.5|datetimeformat(format=format)")
        .unwrap();

    assert_eq!(
        expr.eval(context!(format => "short"), &[])
            .unwrap()
            .to_string(),
        "2023-06-24 18:37"
    );
    assert_eq!(
        expr.eval(context!(format => "medium"), &[])
            .unwrap()
            .to_string(),
        "Jun 24 2023 18:37"
    );
    assert_eq!(
        expr.eval(context!(format => "long"), &[])
            .unwrap()
            .to_string(),
        "June 24 2023 18:37:22"
    );
    assert_eq!(
        expr.eval(context!(format => "full"), &[])
            .unwrap()
            .to_string(),
        "Saturday, June 24 2023 18:37:22.5"
    );
    assert_eq!(
        expr.eval(context!(format => "unix"), &[])
            .unwrap()
            .to_string(),
        "1687624642"
    );
    assert_eq!(
        expr.eval(context!(format => "iso"), &[])
            .unwrap()
            .to_string(),
        "2023-06-24T18:37:22+02:00"
    );

    let expr = env
        .compile_expression("1687624642|datetimeformat(tz='Europe/Moscow')")
        .unwrap();
    assert_eq!(expr.eval((), &[]).unwrap().to_string(), "19:37");
}

#[test]
fn test_datetimeformat_iso_negative() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "America/Chicago");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("1687624642.5|datetimeformat(format='iso')")
        .unwrap();
    assert_eq!(
        expr.eval((), &[]).unwrap().to_string(),
        "2023-06-24T11:37:22-05:00"
    )
}

#[test]
fn test_datetimeformat_time_rs() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "Europe/Vienna");
    env.add_global("DATETIME_FORMAT", "[hour]:[minute]");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("d|datetimeformat(format=format)")
        .unwrap();

    let d = time::OffsetDateTime::from_unix_timestamp(1687624642).unwrap();
    assert_eq!(
        expr.eval(context!(d, format => "short"), &[])
            .unwrap()
            .to_string(),
        "2023-06-24 18:37"
    );
}

#[test]
fn test_datetimeformat_chrono() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "Europe/Vienna");
    env.add_global("DATETIME_FORMAT", "[hour]:[minute]");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("d|datetimeformat(format=format)")
        .unwrap();

    let d = chrono::DateTime::parse_from_rfc3339("2023-06-24T16:37:00Z").unwrap();
    assert_eq!(
        expr.eval(context!(d, format => "short"), &[])
            .unwrap()
            .to_string(),
        "2023-06-24 18:37"
    );
}

#[test]
fn test_dateformat() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "Europe/Vienna");
    env.add_global("DATE_FORMAT", "[year]-[month]");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("1687624642.5|dateformat(format=format)")
        .unwrap();

    assert_eq!(
        expr.eval(context!(format => "short"), &[])
            .unwrap()
            .to_string(),
        "2023-06-24"
    );
    assert_eq!(
        expr.eval(context!(format => "medium"), &[])
            .unwrap()
            .to_string(),
        "Jun 24 2023"
    );
    assert_eq!(
        expr.eval(context!(format => "long"), &[])
            .unwrap()
            .to_string(),
        "June 24 2023"
    );
    assert_eq!(
        expr.eval(context!(format => "full"), &[])
            .unwrap()
            .to_string(),
        "Saturday, June 24 2023"
    );

    let expr = env
        .compile_expression("1687624642|dateformat(tz='Europe/Moscow')")
        .unwrap();
    assert_eq!(expr.eval((), &[]).unwrap().to_string(), "2023-06");
}

#[test]
fn test_dateformat_time_rs() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "Europe/Vienna");
    env.add_global("DATE_FORMAT", "[year]-[month]");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("d|dateformat(format=format)")
        .unwrap();

    let d = time::Date::from_ordinal_date(2023, 42).unwrap();
    assert_eq!(
        expr.eval(context!(d, format => "short"), &[])
            .unwrap()
            .to_string(),
        "2023-02-11"
    );
}

#[test]
fn test_dateformat_chrono_rs() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "Europe/Vienna");
    env.add_global("DATE_FORMAT", "[year]-[month]");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("d|dateformat(format=format)")
        .unwrap();

    let d = chrono::NaiveDate::from_num_days_from_ce_opt(739073);
    assert_eq!(
        expr.eval(context!(d, format => "short"), &[])
            .unwrap()
            .to_string(),
        "2024-07-06"
    );

    assert_eq!(
        expr.eval(context!(d => "2024-07-06", format => "short"), &[])
            .unwrap()
            .to_string(),
        "2024-07-06"
    );
}

#[test]
fn test_datetime_format_naive() {
    let mut env = minijinja::Environment::new();
    minijinja_contrib::add_to_environment(&mut env);

    let d = time::Date::from_calendar_date(2024, time::Month::January, 18)
        .unwrap()
        .with_hms(0, 1, 2)
        .unwrap();

    let expr = env
        .compile_expression("d|datetimeformat(format=format, tz='Europe/Brussels')")
        .unwrap();
    assert_eq!(
        expr.eval(
            context!(d => d.format(&Iso8601::DATE_TIME).unwrap(), format => "iso"),
            &[]
        )
        .unwrap()
        .to_string(),
        "2024-01-18T00:01:02+01:00"
    );
    assert_eq!(
        expr.eval(context!(d, format => "iso"), &[])
            .unwrap()
            .to_string(),
        "2024-01-18T00:01:02+01:00"
    );
}

#[test]
fn test_timeformat() {
    let mut env = minijinja::Environment::new();
    env.add_global("TIMEZONE", "Europe/Vienna");
    env.add_global("TIME_FORMAT", "[hour]:[minute]");
    minijinja_contrib::add_to_environment(&mut env);

    let expr = env
        .compile_expression("1687624642.5|timeformat(format=format)")
        .unwrap();

    assert_eq!(
        expr.eval(context!(format => "short"), &[])
            .unwrap()
            .to_string(),
        "18:37"
    );
    assert_eq!(
        expr.eval(context!(format => "medium"), &[])
            .unwrap()
            .to_string(),
        "18:37"
    );
    assert_eq!(
        expr.eval(context!(format => "long"), &[])
            .unwrap()
            .to_string(),
        "18:37:22"
    );
    assert_eq!(
        expr.eval(context!(format => "full"), &[])
            .unwrap()
            .to_string(),
        "18:37:22.5"
    );
    assert_eq!(
        expr.eval(context!(format => "unix"), &[])
            .unwrap()
            .to_string(),
        "1687624642"
    );
    assert_eq!(
        expr.eval(context!(format => "iso"), &[])
            .unwrap()
            .to_string(),
        "2023-06-24T18:37:22+02:00"
    );

    let expr = env
        .compile_expression("1687624642|timeformat(tz='Europe/Moscow')")
        .unwrap();
    assert_eq!(expr.eval((), &[]).unwrap().to_string(), "19:37");
}
