use indexmap::IndexMap;
use std::sync::OnceLock;

/// `zone.tab` and `iso3166.tab` from IANA tzdata 2025b, the release bundled by chrono-tz 0.10.4:
/// <https://data.iana.org/time-zones/releases/tzdata2025b.tar.gz>
const ZONE_TAB: &str = include_str!("zone.tab");
const ISO3166_TAB: &str = include_str!("iso3166.tab");

/// The `(country code, zone name)` pairs of `zone.tab`, in file order.
fn zone_tab_rows() -> impl Iterator<Item = (&'static str, &'static str)> {
    ZONE_TAB
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let code = fields.next()?;
            let zone = fields.nth(1)?;
            Some((code, zone))
        })
}

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
        let mut names: Vec<&'static str> = zone_tab_rows()
            .map(|(_, zone)| zone)
            .chain(EXTRA_COMMON_TIMEZONES.iter().copied())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    })
}

/// `pytz.country_timezones`: each country code mapped to its zones, in `zone.tab` order:
/// <https://github.com/stub42/pytz/blob/e3c6fe223abf0ad1c1c4e40a9bd319a87bb3b829/src/pytz/__init__.py#L367-L384>
pub(super) fn country_timezones() -> &'static IndexMap<&'static str, Vec<&'static str>> {
    static COUNTRY_TIMEZONES: OnceLock<IndexMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    COUNTRY_TIMEZONES.get_or_init(|| {
        let mut countries: IndexMap<&'static str, Vec<&'static str>> = IndexMap::new();
        for (code, zone) in zone_tab_rows() {
            countries.entry(code).or_default().push(zone);
        }
        countries
    })
}

/// `pytz.country_names`: each country code mapped to its English name, in `iso3166.tab` order:
/// <https://github.com/stub42/pytz/blob/e3c6fe223abf0ad1c1c4e40a9bd319a87bb3b829/src/pytz/__init__.py#L396-L408>
pub(super) fn country_names() -> &'static IndexMap<&'static str, &'static str> {
    static COUNTRY_NAMES: OnceLock<IndexMap<&'static str, &'static str>> = OnceLock::new();
    COUNTRY_NAMES.get_or_init(|| {
        ISO3166_TAB
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_once('\t'))
            .map(|(code, name)| (code, name.trim()))
            .collect()
    })
}
