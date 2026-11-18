//! `dbt deps` package installation and `dbt deps --add-package`.

use dbt_telemetry::{DepsAddPackage, DepsAllPackagesInstalled, DepsPackageInstalled, PackageType};
use indoc::indoc;

use super::support::{Case, Clock, DBT_LOG, TERMINAL, TERMINAL_QUIET, TEXT, all_variants};
use crate::tracing::{emit::create_info_span, span_info::record_span_status};

pub(super) fn cases(cases: &mut Vec<Case>) {
    all_packages_installed(cases);
    package_installed(cases);
    package_added(cases);
}

fn package_types() -> [PackageType; 6] {
    all_variants!(PackageType {
        Unspecified,
        Hub,
        Git,
        Local,
        Private,
        Tarball,
    })
}

/// Emits `attrs` as a span that took 640ms and ended with `error`, if any.
fn span(clock: &Clock, attrs: impl Into<dbt_tracing::TelemetryAttributes>, error: Option<&str>) {
    let span = create_info_span(attrs);
    clock.advance_ms(640);
    if let Some(error) = error {
        record_span_status(&span, Some(error));
    }
    drop(span);
}

fn all_packages_installed(cases: &mut Vec<Case>) {
    let installed = |package_count: u64| {
        Case::new(format!("{package_count} packages"))
            .run(move |clock| span(clock, DepsAllPackagesInstalled { package_count }, None))
    };

    cases.push(
        installed(0)
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Installing packages
                    03:04:06.318901 [info ]:  Installed 0 package
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <green+bold> Installed</> 0 package
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <green+bold>Installing</> packages
                    <green+bold> Installed</> 0 package
                "},
            ),
    );

    cases.push(
        installed(1)
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Installing packages
                    03:04:06.318901 [info ]:  Installed 1 package
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <green+bold> Installed</> 1 package
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <green+bold>Installing</> packages
                    <green+bold> Installed</> 1 package
                "},
            ),
    );

    cases.push(
        installed(3)
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Installing packages
                    03:04:06.318901 [info ]:  Installed 3 packages
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <green+bold> Installed</> 3 packages
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <green+bold>Installing</> packages
                    <green+bold> Installed</> 3 packages
                "},
            ),
    );
}

fn package_installed(cases: &mut Vec<Case>) {
    let package = |package_type: PackageType,
                   name: Option<&str>,
                   url_or_path: Option<&str>,
                   version: Option<&str>| DepsPackageInstalled {
        package_name: name.map(str::to_string),
        package_type: package_type as i32,
        package_version: version.map(str::to_string),
        package_url_or_path: url_or_path.map(str::to_string),
        dbt_core_event_code: String::new(),
    };

    for package_type in package_types() {
        let installed = |label: &str, name: Option<&str>, version: Option<&str>| {
            let installed = package(
                package_type,
                name,
                Some("https://github.com/dbt-labs/dbt-utils.git"),
                version,
            );
            Case::new(format!("{package_type:?}, {label}"))
                .run(move |clock| span(clock, installed.clone(), None))
        };
        let named = installed("named", Some("dbt_utils"), Some("1.3.0"));
        let named_no_version = installed("named, no version", Some("dbt_utils"), None);
        let url_only = installed("url only", None, None);

        cases.extend(match package_type {
            PackageType::Unspecified => [
                named
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing unknown
                            03:04:06.318901 [info ]:  Installed unknown
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> unknown
                            <green+bold> Installed</> unknown
                        "},
                    ),
                named_no_version
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing unknown
                            03:04:06.318901 [info ]:  Installed unknown
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> unknown
                            <green+bold> Installed</> unknown
                        "},
                    ),
                url_only
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing unknown
                            03:04:06.318901 [info ]:  Installed unknown
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> unknown
                            <green+bold> Installed</> unknown
                        "},
                    ),
            ],
            PackageType::Hub => [
                named
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing dbt_utils
                            03:04:06.318901 [info ]:  Installed dbt_utils: 1.3.0
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> dbt_utils
                            <green+bold> Installed</> dbt_utils: 1.3.0
                        "},
                    ),
                named_no_version
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing dbt_utils
                            03:04:06.318901 [info ]:  Installed dbt_utils: latest
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> dbt_utils
                            <green+bold> Installed</> dbt_utils: latest
                        "},
                    ),
                url_only
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing unknown
                            03:04:06.318901 [info ]:  Installed unknown: latest
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> unknown
                            <green+bold> Installed</> unknown: latest
                        "},
                    ),
            ],
            // These types are shown the same way.
            PackageType::Git | PackageType::Local | PackageType::Private | PackageType::Tarball => [
                named
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing dbt_utils
                            03:04:06.318901 [info ]:  Installed dbt_utils from 1.3.0
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> dbt_utils
                            <green+bold> Installed</> dbt_utils from 1.3.0
                        "},
                    ),
                named_no_version
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing dbt_utils
                            03:04:06.318901 [info ]:  Installed dbt_utils
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> dbt_utils
                            <green+bold> Installed</> dbt_utils
                        "},
                    ),
                url_only
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Installing https://github.com/dbt-labs/dbt-utils.git
                            03:04:06.318901 [info ]:  Installed https://github.com/dbt-labs/dbt-utils.git
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>Installing</> https://github.com/dbt-labs/dbt-utils.git
                            <green+bold> Installed</> https://github.com/dbt-labs/dbt-utils.git
                        "},
                    ),
            ],
        });
    }

    let anonymous = package(PackageType::Local, None, None, None);
    cases.push(
        Case::new("no name and no path")
            .run(move |clock| span(clock, anonymous.clone(), None))
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Installing unknown
                    03:04:06.318901 [info ]:  Installed unknown
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <green+bold>Installing</> unknown
                    <green+bold> Installed</> unknown
                "},
            ),
    );

    for (package_type, dbt_log, terminal, text) in [
        (
            PackageType::Hub,
            indoc! {"
                03:04:05.678901 [info ]: Installing dbt_utils
                03:04:06.318901 [info ]: Failed dbt_utils: 1.3.0
            "},
            indoc! {"
                <red+bold>Failed</> dbt_utils: 1.3.0
            "},
            indoc! {"
                <green+bold>Installing</> dbt_utils
                <red+bold>Failed</> dbt_utils: 1.3.0
            "},
        ),
        (
            PackageType::Git,
            indoc! {"
                03:04:05.678901 [info ]: Installing dbt_utils
                03:04:06.318901 [info ]: Failed dbt_utils from 1.3.0
            "},
            indoc! {"
                <red+bold>Failed</> dbt_utils from 1.3.0
            "},
            indoc! {"
                <green+bold>Installing</> dbt_utils
                <red+bold>Failed</> dbt_utils from 1.3.0
            "},
        ),
    ] {
        let failed = package(package_type, Some("dbt_utils"), None, Some("1.3.0"));
        cases.push(
            Case::new(format!("{package_type:?}, failed"))
                .run(move |clock| span(clock, failed.clone(), Some("network unreachable")))
                .expect(DBT_LOG, dbt_log)
                .expect(TERMINAL, terminal)
                .expect(TEXT, text),
        );
    }

    let hub = package(PackageType::Hub, Some("dbt_utils"), None, Some("1.3.0"));
    cases.push(
        Case::new("under the progress bar")
            .run(move |clock| {
                let all = create_info_span(DepsAllPackagesInstalled { package_count: 1 });
                all.in_scope(|| span(clock, hub.clone(), None));
                drop(all);
            })
            .expect(
                TERMINAL,
                indoc! {"
                    <green+bold> Installed</> dbt_utils: 1.3.0
                    <green+bold> Installed</> 1 package
                "},
            )
            .expect_no_output(TERMINAL_QUIET),
    );
}

fn package_added(cases: &mut Vec<Case>) {
    for package_type in package_types() {
        let added = |version: Option<&str>| {
            let added = DepsAddPackage {
                package_name: "dbt-labs/dbt_utils".to_string(),
                package_type: package_type as i32,
                package_version: version.map(str::to_string),
                dbt_core_event_code: String::new(),
            };
            Case::new(format!("{package_type:?}, version {version:?}"))
                .run(move |clock| span(clock, added.clone(), None))
        };
        let versioned = added(Some("1.3.0"));
        let unversioned = added(None);

        cases.extend(match package_type {
            PackageType::Unspecified => [
                versioned
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]:     Adding dbt-labs/dbt_utils
                            03:04:06.318901 [info ]:      Added dbt-labs/dbt_utils
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>    Adding</> dbt-labs/dbt_utils
                            <green+bold>     Added</> dbt-labs/dbt_utils
                        "},
                    ),
                unversioned
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]:     Adding dbt-labs/dbt_utils
                            03:04:06.318901 [info ]:      Added dbt-labs/dbt_utils
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>    Adding</> dbt-labs/dbt_utils
                            <green+bold>     Added</> dbt-labs/dbt_utils
                        "},
                    ),
            ],
            PackageType::Hub => [
                versioned
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]:     Adding dbt-labs/dbt_utils
                            03:04:06.318901 [info ]:      Added dbt-labs/dbt_utils: 1.3.0
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>    Adding</> dbt-labs/dbt_utils
                            <green+bold>     Added</> dbt-labs/dbt_utils: 1.3.0
                        "},
                    ),
                unversioned
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]:     Adding dbt-labs/dbt_utils
                            03:04:06.318901 [info ]:      Added dbt-labs/dbt_utils: latest
                        "},
                    )
                    .expect(
                        TEXT,
                        indoc! {"
                            <green+bold>    Adding</> dbt-labs/dbt_utils
                            <green+bold>     Added</> dbt-labs/dbt_utils: latest
                        "},
                    ),
            ],
            // These types are shown the same way.
            PackageType::Git | PackageType::Local | PackageType::Private | PackageType::Tarball => {
                [
                    versioned
                        .expect(
                            DBT_LOG,
                            indoc! {"
                            03:04:05.678901 [info ]:     Adding dbt-labs/dbt_utils
                            03:04:06.318901 [info ]:      Added dbt-labs/dbt_utils from 1.3.0
                        "},
                        )
                        .expect(
                            TEXT,
                            indoc! {"
                            <green+bold>    Adding</> dbt-labs/dbt_utils
                            <green+bold>     Added</> dbt-labs/dbt_utils from 1.3.0
                        "},
                        ),
                    unversioned
                        .expect(
                            DBT_LOG,
                            indoc! {"
                            03:04:05.678901 [info ]:     Adding dbt-labs/dbt_utils
                            03:04:06.318901 [info ]:      Added dbt-labs/dbt_utils
                        "},
                        )
                        .expect(
                            TEXT,
                            indoc! {"
                            <green+bold>    Adding</> dbt-labs/dbt_utils
                            <green+bold>     Added</> dbt-labs/dbt_utils
                        "},
                        ),
                ]
            }
        });
    }

    let failed = DepsAddPackage {
        package_name: "dbt-labs/dbt_utils".to_string(),
        package_type: PackageType::Hub as i32,
        package_version: Some("9.9.9".to_string()),
        dbt_core_event_code: String::new(),
    };
    cases.push(
        Case::new("failed")
            .run(move |clock| span(clock, failed.clone(), Some("no such version")))
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]:     Adding dbt-labs/dbt_utils
                    03:04:06.318901 [info ]: Failed dbt-labs/dbt_utils: 9.9.9
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <green+bold>    Adding</> dbt-labs/dbt_utils
                    <red+bold>Failed</> dbt-labs/dbt_utils: 9.9.9
                "},
            )
            .expect_no_output(TERMINAL_QUIET),
    );
}
