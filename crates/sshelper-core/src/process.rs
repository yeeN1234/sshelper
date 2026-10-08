use std::process::Command;

/// `Command::new` that does not flash a console window when called from the
/// GUI build on Windows.
pub(crate) fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Last non-empty lines of a tool's output, for error messages.
pub(crate) fn tail(bytes: &[u8], lines: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let collected: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    collected[collected.len().saturating_sub(lines)..].join("\n")
}
