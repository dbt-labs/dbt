//! Own test binary: `freeze_now` is process-wide and must not leak into other tests.
#![cfg(all(feature = "datetime", feature = "timezone"))]

use chrono::{TimeZone, Utc};
use minijinja::{context, Environment};
use minijinja_contrib::modules::py_datetime::freeze_now;

fn render(template: &str) -> String {
    let mut env = Environment::new();
    minijinja_contrib::add_to_environment(&mut env);
    env.render_str(template, context! {}, &[]).unwrap()
}

/// Yesterday's UTC date, derived from the wall clock.
const YESTERDAY: &str = "{{ (modules.datetime.datetime.utcnow() - \
     modules.datetime.timedelta(days=1)).strftime('%Y-%m-%d') }}";

#[test]
fn frozen_clock_pins_every_wall_clock_entry_point() {
    let real_today = Utc::now().format("%Y-%m-%d").to_string();

    // Just after midnight UTC: a recording made at this instant rendered the previous day.
    freeze_now(Some(Utc.with_ymd_and_hms(2026, 10, 9, 0, 0, 24).unwrap()));
    assert_eq!(render(YESTERDAY), "2026-10-08");
    assert_eq!(
        render("{{ modules.datetime.datetime.utcnow().strftime('%Y-%m-%dT%H:%M:%S') }}"),
        "2026-10-09T00:00:24"
    );
    assert_eq!(
        render(
            "{{ modules.datetime.datetime.now(modules.pytz.utc).strftime('%Y-%m-%dT%H:%M:%S') }}"
        ),
        "2026-10-09T00:00:24"
    );
    // `today()` and `date.today()` read the local calendar day of the frozen instant.
    let local_day = Utc
        .with_ymd_and_hms(2026, 10, 9, 0, 0, 24)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string();
    assert_eq!(
        render("{{ modules.datetime.datetime.today().strftime('%Y-%m-%d') }}"),
        local_day
    );
    assert_eq!(
        render("{{ modules.datetime.date.today().strftime('%Y-%m-%d') }}"),
        local_day
    );

    // Unfreezing restores the system clock.
    freeze_now(None);
    assert_eq!(
        render("{{ modules.datetime.datetime.utcnow().strftime('%Y-%m-%d') }}"),
        real_today
    );
}
