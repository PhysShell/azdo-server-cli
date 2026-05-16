//! Launch the system browser. Shared by the `--open` CLI flag and the
//! TUI `o` hotkey.

use std::process::Command;

use crate::error::{AzdoError, AzdoResult};

/// Build the platform command that opens `url` in the default browser,
/// without spawning it (kept separate so it is testable).
fn opener_command(url: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    } else {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(url);
        cmd
    }
}

/// Open `url` in the system browser. The child is reaped immediately
/// (`start` / `xdg-open` detach and exit at once) so this never blocks
/// and leaves no zombie behind.
pub(crate) fn open(url: &str) -> AzdoResult<()> {
    let mut child = opener_command(url)
        .spawn()
        .map_err(|e| AzdoError::Browser(e.to_string()))?;
    drop(child.wait());
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use super::opener_command;

    const URL: &str = "https://azdo.local/tfs/Col/Proj/_workitems/edit/42";

    #[test]
    fn opener_command_targets_the_url() {
        let cmd = opener_command(URL);
        let program = cmd.get_program().to_str().expect("program is utf-8");
        let args: Vec<&str> = cmd
            .get_args()
            .map(|a| a.to_str().expect("arg is utf-8"))
            .collect();

        if cfg!(windows) {
            assert_eq!(program, "cmd", "windows uses cmd");
            assert_eq!(
                args,
                ["/C", "start", "", URL],
                "windows passes an empty title then the url to `start`",
            );
        } else {
            assert_eq!(program, "xdg-open", "non-windows uses xdg-open");
            assert_eq!(args, [URL], "xdg-open receives the url verbatim");
        }
    }
}
