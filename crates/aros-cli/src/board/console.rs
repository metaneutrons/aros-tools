//! Validated serial-console planning and execution for configured boards.

use clap::ValueEnum;
use miette::Result;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConsoleProgram {
    /// Pick the first available program in a predictable order.
    Auto,
    Picocom,
    Screen,
    Minicom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolePlan {
    pub program: String,
    pub args: Vec<OsString>,
}

impl ConsolePlan {
    #[must_use]
    pub fn display(&self) -> String {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().map(shell_word))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn plan(requested: ConsoleProgram, device: &Path, baud: u32) -> Result<ConsolePlan> {
    if baud == 0 {
        miette::bail!("Serial baud rate must be greater than zero.");
    }
    if !device.is_absolute() {
        miette::bail!(
            "Serial device '{}' must be an absolute path.",
            device.display()
        );
    }

    let program = match requested {
        ConsoleProgram::Auto => detect_available_program().ok_or_else(|| {
            miette::miette!(
                "No supported serial terminal was found. Install picocom, screen, or minicom, or pass --program with one that is available."
            )
        })?,
        program => program,
    };

    let args = match program {
        ConsoleProgram::Auto => unreachable!("auto is resolved above"),
        ConsoleProgram::Picocom => vec![
            OsString::from("--baud"),
            OsString::from(baud.to_string()),
            device.as_os_str().to_os_string(),
        ],
        ConsoleProgram::Screen => vec![
            device.as_os_str().to_os_string(),
            OsString::from(baud.to_string()),
        ],
        ConsoleProgram::Minicom => vec![
            OsString::from("-D"),
            device.as_os_str().to_os_string(),
            OsString::from("-b"),
            OsString::from(baud.to_string()),
        ],
    };

    Ok(ConsolePlan {
        program: program_name(program).to_string(),
        args,
    })
}

pub fn run(plan: &ConsolePlan) -> Result<()> {
    Ok(crate::observability::run_interactive_command(
        Command::new(&plan.program).args(&plan.args),
        &format!("serial terminal '{}'", plan.program),
    )?)
}

pub fn detect_available_program() -> Option<ConsoleProgram> {
    [
        ConsoleProgram::Picocom,
        ConsoleProgram::Screen,
        ConsoleProgram::Minicom,
    ]
    .into_iter()
    .find(|program| which::which(program_name(*program)).is_ok())
}

#[must_use]
pub const fn program_name(program: ConsoleProgram) -> &'static str {
    match program {
        ConsoleProgram::Auto => "auto",
        ConsoleProgram::Picocom => "picocom",
        ConsoleProgram::Screen => "screen",
        ConsoleProgram::Minicom => "minicom",
    }
}

fn shell_word(word: &OsString) -> String {
    let rendered = word.to_string_lossy();
    if rendered.contains(char::is_whitespace) {
        format!("'{}'", rendered.replace('\'', "'\\''"))
    } else {
        rendered.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{plan, ConsoleProgram};
    use std::path::Path;

    #[test]
    fn screen_plan_is_the_plain_device_and_baud_invocation() {
        let plan = plan(
            ConsoleProgram::Screen,
            Path::new("/dev/cu.usbserial-test"),
            115_200,
        )
        .expect("console plan");

        assert_eq!(plan.program, "screen");
        assert_eq!(plan.display(), "screen /dev/cu.usbserial-test 115200");
    }

    #[test]
    fn picocom_plan_preserves_paths_with_spaces_in_display_output() {
        let plan = plan(
            ConsoleProgram::Picocom,
            Path::new("/dev/serial test"),
            9_600,
        )
        .expect("console plan");

        assert_eq!(plan.display(), "picocom --baud 9600 '/dev/serial test'");
    }
}
