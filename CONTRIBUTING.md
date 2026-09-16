# Contributing to dbt

There are many ways to contribute to the ongoing development of dbt, including code, discussions, and issues. We encourage you to first read our higher-level document: "[Expectations for Open Source Contributors](https://docs.getdbt.com/community/resources/oss-expectations)".

### Notes
* **Adapters** - All adapter jinja logic is maintained in this repository. Please open up an issue here.
* **Database Drivers** - dbt uses the next generation ADBC protocol to submit queries. 
  * **For Snowflake** - please see the [Go Arrow Snowflake Driver](https://github.com/apache/arrow-adbc/tree/main/go/adbc/driver/snowflake)
  * **For BigQuery** - please see the [Go Arrow BigQuery Driver](https://github.com/apache/arrow-adbc/tree/main/go/adbc/driver/bigquery)
  * **For Postgres** - please see the [C Arrow Postgres Driver](https://github.com/dbt-labs/arrow-adbc/tree/main/c/driver/postgresql)
  
* **Branches** - All pull requests from community contributors should target the main branch (default). If the change is needed as a patch for a minor version of dbt that has already been released (or is already a release candidate), a maintainer will backport the changes in your PR to the release branch.
* **Releases** - New releases will include fixes and new features. Before filing a bug, please ensure that you have the latest release installed. 

### Setting up an environment

#### Tools
dbt is written in Rust. Please make sure that you have the [rust toolchain installed](https://www.rust-lang.org/tools/install), along with the preferred testing utility [Nextest](https://nexte.st/). 

1. [Install Rust](https://www.rust-lang.org/tools/install)
2. Install the testing framework used for all tests & testbench configuration [Nextest](https://nexte.st/docs/installation/pre-built-binaries/)
3. Clone the repository `git clone https://github.com/dbt-labs/dbt-core.git`
4. `cd dbt-core`
5. `cargo build` for a debug build. `cargo build --release` for a release build

*There are no virtual environments needed!*


## Making a Change to dbt
After you're done making your code change, make sure:
- your code is formatted: `cargo fmt`
- all tests pass: `cargo nextest run`
- all lints pass: `cargo clippy --all-targets --all-features`.
- you have manually verified it works with a local build of dbt against a real project

Then, open a pull request against the current development branch, `main`. A dbt maintainer will triage the PR and, once it's on the right track, assign a reviewer. They may suggest code revisions for style or clarity, or request that you add test(s).

Once your PR has been approved, a maintainer will take it from there and shepherd your changes into dbt. And that's it! Happy developing 🎉

## What happens after you open a pull request

dbt is developed through dbt Labs' internal build and review process, and this repository is kept in sync with it automatically. You don't need to do anything special for this — but it's worth knowing, because it explains the labels, comments, and status checks you'll see on your PR.

1. **Your PR is triaged.** PRs opened by community members are automatically labeled `community`, and during triage they're marked `source:community` to note the change came from an external contributor. A maintainer reviews the change and, once it's on the right track, assigns a reviewer.

2. **The changelog check runs.** If your PR doesn't include a changelog entry, a bot comments to let you know. See [Adding a CHANGELOG Entry](#adding-a-changelog-entry). If a changelog isn't needed, a maintainer can add the `Skip Changelog` label.

3. **CI is approved (fork PRs only).** Because PRs from forks can't safely access repository secrets, a maintainer must add the `ci:approve-public-fork-ci` label before CI runs. **This label is automatically removed every time you push new commits**, so a maintainer will re-approve after each update. This is expected — it isn't a sign that anything is wrong with your change.

4. **CI results and sync status appear on your PR.** Build results surface as the **dbt Labs CI** status check. The review's progress is reflected through `review-status:` labels:

   | Label | Meaning |
   | --- | --- |
   | `review-status: in-review` | Your change synced successfully and is under review. |
   | `review-status: sync-failed` | The sync couldn't be applied (for example, a merge conflict). A maintainer will help resolve it — you may be asked to rebase. |
   | `review-status: merged-upstream` | Your change has been merged and will land in this repository in the next sync. |
   | `review-status: closed-upstream` | The change was closed without merging. |

5. **Your change is merged.** When your change is merged, a bot comments to let you know and closes this PR. Your commit lands in this repository in the next sync. Closing the PR in this way is normal — your contribution has been accepted, not rejected. 🎉

## Adding a CHANGELOG Entry

We use [changie](https://changie.dev) to generate `CHANGELOG` entries. **Note:** Do not edit the `CHANGELOG.md` directly. Your modifications will be lost.

Follow the steps to [install `changie`](https://changie.dev/guide/installation/) for your system.

Once changie is installed and your PR is created for a new feature, simply run the following command and changie will walk you through the process of creating a changelog entry:

```shell
changie new
```

Commit the file that's created and your changelog entry is complete!

You don't need to worry about which `dbt-core` version your change will go into. Just create the changelog entry with `changie`, and open your PR against the `main` branch. All merged changes will be included in the next release of `dbt-core`.  If a changelog is not required, a maintainer can add the label `Skip Changelog` to bypass this requirement.
