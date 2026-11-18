use std::sync::Arc;

use crate::task::{CommandFn, ExecuteAndCompare, ExecuteOnly, TaskSeq, fs_cmd_vec};

/// How a test crate runs `fs` commands: its own CLI feature stack and parser
/// (`command_fn`), the flags it appends to every command (`extra_args`), and the
/// `--static-analysis` mode it adds to commands that don't name one
/// (`default_static_analysis`).
pub struct FsRunner {
    pub command_fn: fn() -> Arc<CommandFn>,
    pub extra_args: &'static [&'static str],
    pub default_static_analysis: Option<&'static str>,
}

fn cmd_vec(runner: &FsRunner, command: &str) -> Vec<String> {
    let mut cmd = fs_cmd_vec(command);
    cmd.extend(runner.extra_args.iter().map(|arg| arg.to_string()));
    cmd
}

/// Appends a step that runs `command` and compares its output to the golden files.
pub fn fs<'a>(seq: &'a mut TaskSeq, runner: &FsRunner, command: &str) -> &'a mut TaskSeq {
    let name = seq.name().to_owned();
    seq.task(Box::new(ExecuteAndCompare::new(
        name,
        cmd_vec(runner, command),
        (runner.command_fn)(),
        false,
    )))
}

/// Appends a step that runs `command` without comparing output; failures are allowed.
pub fn fs_no_golden<'a>(seq: &'a mut TaskSeq, runner: &FsRunner, command: &str) -> &'a mut TaskSeq {
    let name = seq.name().to_owned();
    seq.task(Box::new(
        ExecuteOnly::new(name, cmd_vec(runner, command), (runner.command_fn)(), true)
            .with_allow_failure(true),
    ))
}
