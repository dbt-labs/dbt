use clap::{Parser, Subcommand};
use dbt_ci::{
    BumpCargoVersionArgs, HomebrewPublishArgs, HomebrewRenderArgs, PackArgs, PypiPublishArgs,
};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "cargo-ci",
    bin_name = "cargo ci",
    about = "Release-pipeline commands for dbt-fusion",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Write `[workspace.package].version` and refresh `Cargo.lock`.
    #[command(name = "bump-cargo-version")]
    BumpCargoVersion(BumpCargoVersionArgs),

    /// Homebrew formula commands.
    #[command(subcommand)]
    Homebrew(HomebrewCmd),

    /// Python wheel commands.
    #[command(subcommand)]
    Pypi(PypiCmd),
}

#[derive(Subcommand, Debug)]
enum PypiCmd {
    /// Pack pre-built binaries into per-platform wheels.
    Pack(PackArgs),

    /// Publish wheels from `--dist`, or build and publish the sdist with `--download-base-url`.
    Publish(PypiPublishArgs),
}

#[derive(Subcommand, Debug)]
enum HomebrewCmd {
    /// Render a Homebrew formula from release tarballs.
    Render(HomebrewRenderArgs),

    /// Push a rendered formula to a Homebrew tap repo.
    Publish(HomebrewPublishArgs),
}

fn main() -> ExitCode {
    match Cli::parse().cmd {
        Cmd::BumpCargoVersion(args) => dbt_ci::bump_cargo_version::execute(args),
        Cmd::Homebrew(HomebrewCmd::Render(args)) => dbt_ci::homebrew::render::execute(args),
        Cmd::Homebrew(HomebrewCmd::Publish(args)) => dbt_ci::homebrew::publish::execute(args),
        Cmd::Pypi(PypiCmd::Pack(args)) => dbt_ci::pack::execute(args),
        Cmd::Pypi(PypiCmd::Publish(args)) => dbt_ci::publish::execute(args),
    }
}
