# Announcing… v2.0 (June 2026)

*At last.*

Okay, so you saw the [announcement](https://www.getdbt.com/blog/fivetran-and-dbt-are-one-company-now-here-s-what-that-means), and you read [the dev blog post](https://docs.getdbt.com/blog/dbt-core-v2-is-here), which explains in detail how we got from there (Fusion, new engine, ELv2) to here (it’s just dbt Core, v2.0).

Last fall, we kicked off our big birthday party for the dbt community by saying that dbt was turning “2-dot-oh.”  Now we get to turn to each other, turn back to the audience, winking, “This was our plan all along!”

Except, it wasn’t. At least, not really. We said that in October because Fusion sure had the *feeling* of a new major version, with a ground-up rewrite, and a new stricter spec. But we were trying to build two things in parallel: a fast new SQL-comprehending engine with the latest technologies; and a beat-for-beat rewrite of dbt Core in Rust, conformant with every behavior except the ones we intentionally choose to change. The latter, it turns out, is a lot of work.

It was only through this hard work that we proved the “core” of the Rust rewrite could be — *should* be — dbt Core v2.0. That it no longer made sense to keep living in a bifurcated world, when we can make a much better one for everyone to live in.

Still… we must have had an inkling, right? Heck, it’s been `dbt-fusion==2.0.0.xxx` since the very first release, in May 2025. (I guess the we had the date right, just the year was [OB1](https://en.wikipedia.org/wiki/Off-by-one_error).) Today, the foundational code is available as dbt Core v2.0. *One engine, under the Apache 2.0 license, indivisible, with faster parsing and modern interfaces for all.* 

In the meantime, we actually did the two-engine thing. We triaged bugs and feature requests. We refined them, shipped them, and tested them, *twice,* in both dbt Core v1.12 (Python) and Fusion / v2.0 (Rust). So, regardless of whether you’re still using Core or Fusion today, we’ve got good news for you - the dbt framework is alive and well.

| **Version** | **When** | **Namesake** | **Stuff** |
| --- | --- | --- | --- |
| [v1.12](https://docs.getdbt.com/docs/dbt-versions/core-upgrade/upgrading-to-v1.12) | May (beta) | Alecia Beth Moore-Hart (P!NK) | UDF extensions. Easier specs for semantic layer, and Iceberg `catalogs`. Long-upvoted feature requests: `vars.yml`; `on_error` model config; [mix-and-match yaml selectors](https://docs.getdbt.com/reference/node-selection/methods?version=1.12#selector); `run-operation` for adhoc `—-sql`; jinja file extensions; `latest_version_pointer`. Improved exception handling and error messages. Dozens of bug fixes across core + adapters, including Python 3.14 support. `--use-v2-parser` to try out the speedy new Rust parser before you upgrade to v2.0. |
| v2.0 | June (alpha) |  | Faster Rust rewrite, at full parity with v1. Strict codified language spec. New parquet metadata artifacts, built for speed & scale, powering new dbt-docs. |

## Past

We just hired a bunch of new developers to work on dbt Core. And, thanks to the consequences of our own actions, the Fusion engineering team actually, ergo, ipso facto, works on dbt Core (v2.0) now.

Meaning - *there are several dozen people working on the dbt framework today,* more than ever before.

Over the past six months, we’ve resolved some of the most-upvoted feature requests from the dbt community:

<img width="998" height="423" alt="most-upvoted features requests from the dbt community closed in the past six months" src="https://github.com/user-attachments/assets/b7f675f2-a2f1-492a-aede-199509618e77" />

The v1.12 release goes… kind of hard (humble brag). As part of onboarding new folks to work on the dbt-core codebase, we queued up lots of narrow, well-scoped bugs and paper cuts — “good first issue” type-stuff — and knocked out about 100 of them. And you pitched in too, with dozens of external contributions across dbt-core and adapters.

We also shipped some bigger features. These aren’t all flashy, you won’t see them printed on a billboard on highway 101, but they’re the sort of quality-of-life improvements that matter to the folks who use dbt every day.

To highlight a few of these low-key bangers in more detail:

`latest_version_pointer` : For versioned models, automatically create a view in your DWH that points to the latest version of that model. Make it simple and easy for downstream consumers to stay up-to-date, same as they’d get with `ref('my_model')` within dbt. No need for an extra model or a janky post-hook.

103 👍 109 ❤️ 41 💬, heard chef

`vars.yml` : Define your project variables *outside* of `dbt_project.yml` in a new `vars.yml` file. This allows you to a) reduce bloat in your `dbt_project.yml` file and b) reference variables *within* project-level configurations (e.g. `+schema: "{{ var('schema_name') }}”`). 

This is a historic issue - open for over five years (including two [stale-bot-saves](https://github.com/dbt-labs/dbt-core/issues/2955#issuecomment-2058407588)). Throughout that time, there was deep engagement and iteration from so many of you. We’re happy with where we landed, and ready for the next feature request for vars (should we allow them to be namespaced??). 

`on_error` : When a model errors, control what should happen to downstream nodes.

dbt had a strongly held (un-moveable) default here since the beginning - downstream nodes should be `SKIPPED`. But many of you said loud and clear, “I actually want something different in certain cases.” You told us what those cases are and how you believe dbt should handle them. And now dbt can :)

Look through the [v1.12 upgrade guide](https://docs.getdbt.com/docs/dbt-versions/core-upgrade/upgrading-to-v1.12), and tell us which hit on the changelog Top 100 will be playing on repeat in your stereo.

To restate, all of these framework improvements, landing in Core v1.12 — these are going to be in Core v2.0, too.

Our team has been living this two-engine world first-hand: making the same bug fixes and feature implementations across two codebases, in two different languages and testing frameworks. That duplication of effort has definitely slowed us down, and risks divergence of behavior.

That's why we’re excited to move toward a world where one engine powers both the Core and Fusion distributions. Rather than needing to think and talk about a unified “language” distinct from an underlying “engine” that implements it — our beloved (now, dearly departed) donut diagram — it’s one ****foundation to power it all**.**

## Present

Today, we [announced the first alpha release of dbt Core v2.0](https://docs.getdbt.com/blog/dbt-core-v2-is-here). This new major version is powered by the same Rust rewrite of Core v1 that powers Fusion. All of the code is licensed under Apache 2.0, and you can see it on the `main` branch of the `dbt-core` repo. 

This is the future of the dbt framework:

- fast and built for scale (Rust, ADBC, etc)
- shared foundation - one donut, not two donuts
- single adapter layer - all bundled in
- and one more thing… a brand-new dbt-docs (!) This is a long-awaited refresh (something we talked about in our [very first roadmap post](https://github.com/dbt-labs/dbt-core/blob/main/docs/roadmap/2022-05-dbt-a-core-story.md#epilogue-whats-missing)) - check out [the github discussion](https://github.com/dbt-labs/dbt-core/discussions/13080) for more details

If you want to read more about how our thinking and licensing strategy evolved over the past year, [Joel & Grace wrote a whole blog post](https://docs.getdbt.com/blog/dbt-core-v2-is-here).

**Why alpha?** Isn’t Fusion already in Preview? 

We’ve spent the last 6+ months working hard on Fusion’s “conformance”: given the same project inputs, does it produce the same results as Core v1.X? We fixed thousands of bugs using this method, and while we now feel confident running Fusion in an environment we manage (dbt platform), software loves to find new edge cases out in the big bold world. 

That's why we need YOUR help to run and test v2.0 against the full population of dbt projects. We need YOUR ever-bigger DAGs, YOUR wacky package macros, YOUR bespoke override of the incremental materialization, and YOUR 2009 laptop running Windows Vista.

The best way to help is by [testing out v2.0](https://docs.getdbt.com/docs/local/install-dbt) while it’s in alpha - try running it against your project, and let us know if you encounter any unexpected errors.

Did you know - you can get ready for v2.0, even while you’re still running on dbt Core v1.X?

- In v1.10/v1.11, we introduced deprecation warnings to help you get your project up to the new stricter spec - no more configs being silently ignored on account of being misspelled or in the wrong spot
- In v1.12 - we’ve made the *harder-better-faster-stricter* Rust parser available, behind an opt-in flag. This should make your projects feel *speedy*, no matter how large. **But you gotta be strict, first. Try it out with: `dbt parse --use-v2-parser`.

Fusion has come such a long way in the past 12 months — from a future-tense, lab-tested promise to a present-tense, production-ready, in-the-wild reality — because of the thousands of community members who (out of genuine interest, and generosity of their time) put it through its paces. We think that code is ready to earn the hearts and minds of the entire community, and to take its rightful spot as the next major version of this thing that we’ve all spent the past decade building together.

## Future

### The path from alpha to GA

Our primary focus over the next several months is getting Core v2.0 from alpha to final release. General availability, ready for primetime, anywhere and everywhere — on your CLI, in dbt platform, in someone’s custom dbt runner, all of the above.

We’ll be finding and squashing lots of bugs, as well as closing the last few gaps in parity (such as a [programmatic (Python) interface](https://docs.getdbt.com/reference/programmatic-invocations) for invoking dbt Core v2.0).

If you (like most people) are still running Core v1 in all those places, what does this mean for you?

- You can try out v2.0 (in alpha) today!
- The Python codebase hasn’t gone anywhere. You can see all the code in the `1.latest` [branch](https://github.com/dbt-labs/dbt-core/tree/1.latest). When we need to cut new patch releases of Core v1.12.x, and we surely will, that’s where we’ll do it from.
- Core v1 is not going away tomorrow, or any time soon. We are committed to supporting the vast majority of the community that is running on Core v1 today. We will track adoption on the migration to v2.0, and develop our plans for long-term maintenance of v1 accordingly.
- If you’re an **adapter maintainer:** don’t you worry, check out [this Guide](https://docs.getdbt.com/guides/adapter-creation-v2) about how to contribute support for your favorite DWH to the newly verticalized adapter layer in Core v2.0.

### What’s next for the dbt framework?

We’re going to keep building the dbt framework — new capabilities and improvements of existing ones, on top of the v2.0 engine. As always, we'll be monitoring github for your comments and upvotes, but there are a handful of feature ideas already on our minds:

1. **Model freshness:** Check to see if your models are as fresh as you think they should be, within *and across* projects, so that data product consumers can check if producers are meeting a promised SLA. (And before you ask - yes, we did talk about this in our last roadmap post as a thing we were planning to include in v1.12; we had to make some tricky trade-offs as part of doing the v2.0 effort, and this was one of them. But we’re still interested! If you are too, [give the issue a read](https://github.com/dbt-labs/dbt-core/issues/12719) and let us know what you think.)
2. **Checks:** In [the dbt-docs discussion](https://github.com/dbt-labs/dbt-core/discussions/13080), we describe how we’re powering fast local metadata queries with a new set of well-structured parquet files and an in-process database. Those same files could power *project quality checks* as metadata queries, folded into the task graph, executed during compile / before execution. You could prevent a `dbt build` if a model is missing an owner, messing up a naming convention, or selecting from a source it shouldn’t be. (For any SDF knowers out there, [this should sound familiar](https://docs.sdf.com/guide/data-quality/checks).) Imagine: a [v2.0 of the dbt-project-evaluator package](https://github.com/dbt-labs/dbt-project-evaluator) that ships our best practices as `checks`, written as simple SQL queries (instead of Jinja `{{ graph }}` manipulation that’s as impressive as it is illegible).
3. **Unit testing macros:** [This feature](https://github.com/dbt-labs/dbt-core/issues/10547) would help us (dbt maintainers), adapter developers, external contributors, heroic maintainers of internal packages featuring load-bearing Jinja macros, and even (especially) the agent who hands you 1000 lines of level-9000 Jinja. “Write a unit test for that new macro, please.” The trickiest bit will be finding elegant ways to mock all the stateful inputs to dbt’s Jinja-rendering context: vars, env vars, `{{ target }}`, and (everyone’s favorite) introspection queries.
4. **Installing agent skills from dbt packages**, as part of `dbt deps`: [We’ve already shown](https://github.com/dbt-labs/dbt-agent-skills) how `skills` can be a useful construct for making agents better at dbt in general, putting the standard workflows and best practices into structured prose. We think `skills` are also a mechanism by which users cant instruct agents on the nuances of *their* data, and *their* dbt project. dbt packages are already in the business of shipping around markdown files — we think this could be one more way that users move themselves [up the stack](https://www.getdbt.com/about-us/values#:~:text=We%20believe%20in%20moving%20up%20the%20stack.). [Discussion is open](https://github.com/dbt-labs/dbt-core/discussions/12521).

## **A vision to work towards**

Humans like CLIs. Agents like CLIs. dbt is, first and foremost, a CLI. We think all dbt capabilities should be possible from the dbt CLI.

By that we mean *all* capabilities of dbt — open source and proprietary, local and remote — should have an entry point in the CLI.

The start is: `dbt login` for connected platform features. We're planning to add this in the next dbt Core v1.X release (v1.13), to smooth the onboarding path for folks who want to use [dbt State](https://docs.getdbt.com/docs/deploy/dbt-state-about). `dbt login` is already in dbt Core v2 and in Fusion, where it will also get you access to Fusion-only things like advanced SQL comprehension, linting, and column-level lineage.

You do not need to use those features. You do not need to run `dbt login` if you don’t want to. The code in `dbt-core` to power those features is in thin clients, all licensed under Apache 2.0. But when you run `dbt --help`, you will start seeing the new `login` command.

In a similar vein, with the archival of the `dbt-fusion` repo, and the consolidation of previously-separate components (adapters) into a monorepo — we’d like to start using this GitHub repo (`dbt-core`) for discussions that span across a larger surface area because we want your feedback on *all* dbt features.

We want to acknowledge that *this is a change.* It’s a change that feels good to us, because it’s a way to give the community easier access to all parts of dbt, and to maintain *fewer better channels* for product feedback. But that change might feel weird for some of you. If that’s you, we want to talk about it - DM us in dbt community slack (really).

## Mergin’

Today is a big day for the dbt community:

- We’ve merged two companies (Fivetran + dbt Labs) into one (name TBD!)
- We’re merging two engines (Fusion + Core) into one (Core v2.0)

We are once again committing to true-blue Apache-2 OSS as the foundation, and the future, of dbt.

Our request for all of you:

- be excited with us and talk about it
- try this all out and tell us how to make it better
- as we consolidate repos, help us make sure no long-loved issue is forgotten
- shape the future of dbt by joining [discussions on GitHub](https://github.com/dbt-labs/dbt-core/discussions) 
- and, hey, [come hangout in Vegas this September](https://www.getdbt.com/dbt-summit/registration)

HAGS (have a great summer),

JEG (jerco elias grace)