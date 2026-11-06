use std::sync::OnceLock;

/// `zone.tab` from IANA tzdata 2025b, the release bundled by chrono-tz 0.10.4:
/// <https://data.iana.org/time-zones/releases/tzdata2025b.tar.gz>
const ZONE_TAB: &str = include_str!("zone.tab");

/// Names pytz adds to `common_timezones` on top of `zone.tab`.
const EXTRA_COMMON_TIMEZONES: &[&str] = &[
    "UTC",
    "GMT",
    "US/Eastern",
    "US/Pacific",
    "US/Mountain",
    "US/Central",
    "US/Arizona",
    "US/Hawaii",
    "US/Alaska",
    "Canada/Newfoundland",
    "Canada/Atlantic",
    "Canada/Eastern",
    "Canada/Central",
    "Canada/Mountain",
    "Canada/Pacific",
];

/// `pytz.common_timezones`, built from `zone.tab` the way pytz builds it in `add_allzones`:
/// <https://github.com/stub42/pytz/blob/e3c6fe223abf0ad1c1c4e40a9bd319a87bb3b829/gen_tzinfo.py#L96-L123>
pub(super) fn common_timezones() -> &'static [&'static str] {
    static COMMON_TIMEZONES: OnceLock<Vec<&'static str>> = OnceLock::new();
    COMMON_TIMEZONES.get_or_init(|| {
        let mut names: Vec<&'static str> = ZONE_TAB
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_whitespace().nth(2))
            .chain(EXTRA_COMMON_TIMEZONES.iter().copied())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    })
}
