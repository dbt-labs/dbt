# dbt: Magic to Do (December 2025)

dbt Core will turn in 10 in March 🥳. That’s almost a decade of `select`-ing, data-testing, and docs-generating.

ICYMI - [we threw a birthday bash for the dbt Community at Coalesce this year](https://www.youtube.com/watch?v=aMUAQjqTKtc), and we used the occasion to reflect on all the ways dbt has grown up:

There are now 2 engines: dbt Core (which will remain Apache 2.0 forever) and dbt Fusion (which uses a [mix of licenses](https://www.getdbt.com/blog/new-code-new-license-understanding-the-new-license-for-the-dbt-fusion-engine))...

...And lots more Open Source code: [MetricFlow](https://www.getdbt.com/blog/open-source-metricflow-governed-metrics) (now Apache 2.0!), [Fusion adapters](https://github.com/dbt-labs/dbt-fusion), [dbt-jinja](https://github.com/dbt-labs/dbt-fusion/tree/main/crates/dbt-jinja), [jsonschemas](https://github.com/dbt-labs/dbt-jsonschema/tree/main/schemas/latest_fusion), and [dbt-autofix](https://github.com/dbt-labs/dbt-autofix).

The language is codified: `dbt-jinja` for the Jinja you can use in dbt, `jsonschemas` for yaml...

...And will continue to grow, [with new features across Core and Fusion](https://www.youtube.com/watch?v=6lI9d-gPKW8).

As always, there is more ~~work~~ magic to do.

# 2025 Wrapped

In 2025, we had: 

- **>90k** weekly active projects
- **>10k** comments across over **>5k** issues from over **>1k** people across our open source and source available repos
- **4** new product-led discussions:
    - [Sample Mode (for faster Development and CI 🚀)](https://github.com/dbt-labs/dbt-core/discussions/11200)
    - [Out-of-the-box support for UDFs](https://github.com/dbt-labs/dbt-core/discussions/11851)
    - [UX Feedback on Logging and stdout](https://github.com/dbt-labs/dbt-fusion/discussions/584)
    - [Source schemas should be first-class, versioned artifacts](https://github.com/dbt-labs/dbt-fusion/discussions/1042)
- **10** [Community Awards](https://www.youtube.com/watch?v=I-DgySJ0Syg)
- **1** [demo on roller skates](https://youtu.be/aMUAQjqTKtc?si=9ZCmz30wZ118HeHI&t=1116)
- **2** minor dbt Core releases
    
    
    | **Version** | **When** | **Namesake** | **Stuff** |
    | --- | --- | --- | --- |
    | [v1.10](https://docs.getdbt.com/docs/dbt-versions/core-upgrade/upgrading-to-v1.10) | June | [Florence Earle Coates](https://github.com/dbt-labs/dbt-core/releases/tag/v1.10.0) | `--sample` mode. Catalogs. Macro argument validation. Calculate source freshness via a custom `loaded_at_query`. YAML `anchors:`. |
    | [v1.11](https://docs.getdbt.com/docs/dbt-versions/core-upgrade/upgrading-to-v1.11) | December | [Juan R. Torruella](https://github.com/dbt-labs/dbt-core/releases/tag/v1.11.0) | User-defined functions. `cluster_by` for dynamic tables. Deprecation warnings using new and improved jsonschemas, enabled by default.  |
- and **1** dbt community <3

### New language features

This past year we added three big new language features to the dbt framework:

- **[sample mode](https://docs.getdbt.com/docs/build/sample-flag)** renders filtered `ref`s and `source`s using time-based sampling, allowing developers to validate outputs without building entire historical datasets.
- **[catalogs](https://docs.getdbt.com/docs/mesh/iceberg/about-catalogs)** abstraction for materializing dbt models as Iceberg tables, with a consistent set of configs (more on this below).
- **[user-defined functions](https://docs.getdbt.com/docs/build/udfs)** as a new dbt resource type - allowing you to define custom functions in your project, call them in models, and register them in your warehouse as part of building your DAG.

All of these are supported now across BOTH engines - dbt Core and dbt Fusion.

The language is also now **codified** - **[new and improved jsonschemas](https://github.com/dbt-labs/dbt-jsonschema/tree/main/schemas/latest_fusion)** power warnings in Core (and errors in Fusion) to help you proactively identify and update deprecated configurations (such as misspelled config keys, old properties, or incorrect data types). 

This stricter spec creates clearer separation between the built-in configurations of the dbt language (or flags/options of the dbt engine) and your custom code (nested under `meta`), reducing the risk of collisions as we continue to add new capabilities and configurations to the dbt framework in the future.

Because we plan to keep building dbt for a long time to come, and for that we need…

### Stability

[Two years ago](https://github.com/dbt-labs/dbt-core/blob/main/docs/roadmap/2023-11-dbt-tng.md), we saw that lots of projects were still running older versions of dbt Core, even as we pushed ahead with newer ones. Our top priority was making it easier to upgrade. To that end, we’ve made significant investments in interface stability, and updated the release cadence to 1 new minor version every 6 months.

All of that work has paid off: **Today, the majority of active dbt Core projects run on the latest minor version.** On December 1, >50% of projects were running on dbt Core `v1.10`, which we released in June. We just released `v1.11` this week, and we expect that the majority of projects will upgrade in the next 6 months.

This means that when we fix bugs in patch releases, or release exciting language features in new minor versions — such as UDFs in `v1.11` — more users can get *immediate* access to those fixes and features than ever before. It also means that more users can benefit from strongly-typed schemas for dbt’s properties and configurations. **And if those users want to try out the new Fusion engine while it’s in preview, they can.**

In order for the Fusion engine to work on your project, your dbt code will need to be compatible with the very latest language spec — which means resolving any deprecation warnings that you would have started seeing in dbt Core `v1.10` and `v1.11`. We have a tool that can help: [**`dbt-autofix`**](https://github.com/dbt-labs/dbt-autofix) scans your dbt project for deprecations, and automatically updates your code to align with the latest spec. 

We closed out this year with "De*bug*-cember" - a month-long bug bash where we squashed 35 long-standing issues across parsing, execution, logging, error messages, and more in the lead up to the final `v1.11` release. (To see the full list, head over to #dbt-core-development in the community Slack.)

# What’s in the Queue for 2026?

Over the next six months, we’re adding a bunch of new team members (say hi to new faces as they pop up in slack and github). With their help, we’re eager to tackle the backlog of bugs and "paper cuts". You can upvote and comment on issues to help us prioritize!

As for bigger features…

In our roadmap post from May, we proposed three possibilities for "[What’s next for dbt-the-language?](https://github.com/dbt-labs/dbt-core/blob/main/docs/roadmap/2025-05-new-engine-same-language.md#whats-next-for-dbt-the-language)": 

1. Out-of-the-box support for UDFs
2. Sources from external catalogs
3. Model freshness checks

We tackled #1 in the `v1.11` release. 

We intend to work on #3 next (github discussion coming soon).

As for #2… [Jeremy says "it’s complicated"](https://www.youtube.com/watch?v=bRJJkeJkUsE&t=1s):

> We are always building dbt within the context of the larger data ecosystem. In summer 2024, Databricks and Snowflake acclaimed Iceberg as the most promising standard for interoperable data storage. Following the flurry of new Iceberg catalogs, we [proposed a spec](https://github.com/dbt-labs/dbt-core/discussions/11171), implemented in dbt Core `v1.10`, to configure those `catalogs` as a place to materialize dbt models.
> 
> Over the past year, the major data warehouses have added support for writing tables to *external* Iceberg catalogs — and even for *synchronizing* metadata from those catalogs (e.g. with a Snowflake [catalog-linked database](https://docs.snowflake.com/en/user-guide/tables-iceberg-catalog-linked-database)) so that you can treat their contents like any other table. This makes it substantially easier to treat Iceberg as [just another implementation detail](https://docs.getdbt.com/blog/icebeg-is-an-implementation-detail), and as the interchange layer for [cross-platform workflows](https://www.getdbt.com/blog/introducing-cross-platform-dbt-mesh).
> 
> It also means that other features of dbt "just work." Earlier this year, we added an [experimental feature](https://github.com/dbt-labs/dbt-labs-experimental-features/tree/main/read-external-iceberg) for registering and refreshing Iceberg tables (à la `dbt-external-sources`), and in our May roadmap we proposed "sources from external catalogs" as a potential new feature of the dbt language. But if the underlying data warehouse can synchronize tables with an external Iceberg catalog automatically, then `sources` pointing to tables in an external Iceberg catalog already work, without any changes to the dbt language. That’s not the full end of the story — Iceberg requires a lot of setup (could dbt help here?) and careful optimization (might it be faster/cheaper to batch-calculate `freshness` by integrating with an Iceberg REST catalog directly, rather than querying the data warehouse?) — it warrants more time and testing. The plan is to keep evolving dbt along with the Iceberg ecosystem in 2026.

Last-but-not-least, we will continue to invest in improving the stability of dbt Core. There’s much more we can improve about:

- the interfaces shared between Core and Fusion
- the integration between dbt Core and MetricFlow, [now that the latter is Apache 2.0 too](https://www.getdbt.com/blog/open-source-metricflow-governed-metrics)
- the ease of contributing to our open source and source-available codebases

# [Join us, leave your fields to flower](https://www.youtube.com/watch?v=AqbYa-NXFOg)

Looking for ways to get involved in the dbt community? 

- give or get help in the [community slack](https://www.getdbt.com/community/join-the-community)
- open up bug reports, feature requests, or discussions in our repos: [dbt-core](https://github.com/dbt-labs/dbt-core), [dbt-adapters](https://github.com/dbt-labs/dbt-adapters) (our new Core adapters monorepo), and [dbt-fusion](https://github.com/dbt-labs/dbt-fusion) are great places to start
- join zoom feedback sessions to help shape new features coming to dbt
- follow along with Fusion progress by [reading our diaries](https://github.com/dbt-labs/dbt-fusion/discussions/categories/announcements) and [subscribing to our linkedin newsletter](https://www.linkedin.com/newsletters/fusion-diaries-7366935294090084360/)
- find community in-person at our [dbt meetups](https://www.meetup.com/pro/dbt/) and [roadshows](https://www.getdbt.com/events), all around the world

See you in the new year,

your neighborhood theater kids (Jerco & Grace)
