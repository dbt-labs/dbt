//! Confirmation prompt for actions that run code we did not ship in-process
//! (external install scripts, external package managers). Shared by
//! [`crate::upgrade`] and, in the future, `dbt system doctor`.

use std::io::IsTerminal;

use dbt_common::{ErrorCode, FsResult, fs_err};

/// Whether stdin is a terminal an interactive prompt could actually be
/// answered on -- shared by [`confirm`] and any other prompt in this crate
/// that needs the same "don't guess, don't hang" gating.
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// Asks the user to confirm `prompt` before proceeding.
///
/// - `assume_yes` (from a command's `--yes`/`-y` flag) skips the prompt
///   entirely and returns `Ok(true)`.
/// - Otherwise, if stdin isn't a terminal, refuses to guess: returns an
///   error rather than hanging on a prompt nobody can answer, or silently
///   declining an action the user may have intended.
/// - Otherwise, prompts interactively, defaulting to "no".
pub fn confirm(prompt: &str, assume_yes: bool) -> FsResult<bool> {
    if assume_yes {
        return Ok(true);
    }

    if !is_interactive() {
        return Err(fs_err!(
            ErrorCode::InvalidArgument,
            "{prompt}\n\nThis is not an interactive terminal, so this action can't be confirmed. \
             Re-run with --yes to proceed automatically."
        ));
    }

    dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()
        .map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "failed to read confirmation prompt: {e}"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_yes_skips_prompt() {
        assert!(confirm("do the thing?", true).unwrap());
    }
}
