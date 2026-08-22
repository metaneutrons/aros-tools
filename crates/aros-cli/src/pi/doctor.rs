use super::config::Board;
use super::console;
use crate::hosttools;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warning,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub status: CheckStatus,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == CheckStatus::Failure)
    }

    pub fn print(&self) {
        for check in &self.checks {
            let marker = match check.status {
                CheckStatus::Pass => "✅",
                CheckStatus::Warning => "⚠️ ",
                CheckStatus::Failure => "❌",
            };
            println!("{marker} {:<18} {}", check.label, check.detail);
        }
    }

    fn push(&mut self, status: CheckStatus, label: &str, detail: impl Into<String>) {
        self.checks.push(DoctorCheck {
            status,
            label: label.to_string(),
            detail: detail.into(),
        });
    }
}

#[must_use]
pub fn inspect(board: &Board, repo_root: &Path) -> DoctorReport {
    let mut report = DoctorReport::default();
    report.push(
        CheckStatus::Pass,
        "board profile",
        format!(
            "{} ({}, {}, preset {})",
            board.name, board.config.model, board.config.transport, board.config.preset
        ),
    );
    match board.config.debug_transport {
        Some(transport) => report.push(CheckStatus::Pass, "debug transport", transport.to_string()),
        None => report.push(
            CheckStatus::Warning,
            "debug transport",
            "not declared; this CLI will not configure a debugger automatically",
        ),
    }

    check_command(&mut report, "cmake", "cmake");
    check_command(&mut report, "cargo", "cargo");
    check_hosttools(&mut report, repo_root);
    check_artifacts(&mut report, board, repo_root);
    check_dtb(&mut report, board, repo_root);
    check_core_kobjs(&mut report, board, repo_root);
    check_tftp_root(&mut report, board);
    check_serial(&mut report, board);
    check_serial_terminal(&mut report);
    report
}

fn check_dtb(report: &mut DoctorReport, board: &Board, repo_root: &Path) {
    match board.rpi4_dtb_path(repo_root, None) {
        Ok(Some(path)) => report.push(
            CheckStatus::Pass,
            "rpi4 dtb",
            format!("{} (passed as AROS_RPI4_DTB)", path.display()),
        ),
        Ok(None) => report.push(
            CheckStatus::Pass,
            "rpi4 dtb",
            "not required for this board model",
        ),
        Err(error) => report.push(CheckStatus::Failure, "rpi4 dtb", error.to_string()),
    }
}

fn check_core_kobjs(report: &mut DoctorReport, board: &Board, repo_root: &Path) {
    match board.rpi4_core_kobj_dir(repo_root, None) {
        Ok(Some(path)) => report.push(
            CheckStatus::Pass,
            "rpi4 core KOBJs",
            format!("{} (passed as AROS_RPI4_CORE_KOBJ_DIR)", path.display()),
        ),
        Ok(None) => report.push(
            CheckStatus::Pass,
            "rpi4 core KOBJs",
            "not required for this board model",
        ),
        Err(error) => report.push(CheckStatus::Failure, "rpi4 core KOBJs", error.to_string()),
    }
}

fn check_command(report: &mut DoctorReport, label: &str, command: &str) {
    match which::which(command) {
        Ok(path) => report.push(CheckStatus::Pass, label, path.display().to_string()),
        Err(_) => report.push(
            CheckStatus::Failure,
            label,
            format!("'{command}' is not available in PATH"),
        ),
    }
}

fn check_hosttools(report: &mut DoctorReport, repo_root: &Path) {
    let hosttools = hosttools::check(repo_root);
    if hosttools.is_complete() {
        report.push(
            CheckStatus::Pass,
            "host tools",
            hosttools.bin_dir.display().to_string(),
        );
    } else {
        let missing = hosttools
            .missing
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy())
            .collect::<Vec<_>>()
            .join(", ");
        report.push(
            CheckStatus::Warning,
            "host tools",
            format!("missing {missing}; `aros build` will build them automatically"),
        );
    }
}

fn check_artifacts(report: &mut DoctorReport, board: &Board, repo_root: &Path) {
    let artifacts = board.artifact_dir(repo_root);
    if artifacts.is_dir() {
        report.push(
            CheckStatus::Pass,
            "artifacts",
            artifacts.display().to_string(),
        );
    } else {
        report.push(
            CheckStatus::Warning,
            "artifacts",
            format!(
                "'{}' does not exist yet; run `aros pi build` first",
                artifacts.display()
            ),
        );
    }
}

fn check_tftp_root(report: &mut DoctorReport, board: &Board) {
    let Ok(root) = board.tftp_root() else {
        report.push(
            CheckStatus::Failure,
            "tftp root",
            "missing or invalid; deploy only publishes into an explicit existing local directory",
        );
        return;
    };
    if root.starts_with("/dev") {
        report.push(
            CheckStatus::Failure,
            "tftp root",
            format!("'{}' is a device path", root.display()),
        );
    } else if root.is_dir() {
        match board.deployment_dir() {
            Ok(destination) => report.push(
                CheckStatus::Pass,
                "tftp root",
                format!("{} → {}", root.display(), destination.display()),
            ),
            Err(error) => report.push(CheckStatus::Failure, "tftp root", error.to_string()),
        }
    } else {
        report.push(
            CheckStatus::Failure,
            "tftp root",
            format!("'{}' does not exist or is not a directory", root.display()),
        );
    }
}

fn check_serial(report: &mut DoctorReport, board: &Board) {
    match board.serial_device() {
        Ok(device) if device.exists() => report.push(
            CheckStatus::Pass,
            "serial device",
            format!("{} @ {} baud", device.display(), board.config.serial_baud),
        ),
        Ok(device) => report.push(
            CheckStatus::Warning,
            "serial device",
            format!("'{}' is not currently present", device.display()),
        ),
        Err(_) => report.push(
            CheckStatus::Warning,
            "serial device",
            "not configured; `aros pi console` needs serial_device or --device",
        ),
    }
}

fn check_serial_terminal(report: &mut DoctorReport) {
    match console::detect_available_program() {
        Some(program) => report.push(
            CheckStatus::Pass,
            "serial terminal",
            console::program_name(program),
        ),
        None => report.push(
            CheckStatus::Warning,
            "serial terminal",
            "install picocom, screen, or minicom before using `aros pi console`",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{inspect, CheckStatus};
    use crate::pi::config::{Board, BoardConfig, Transport};

    #[test]
    fn doctor_flags_a_missing_tftp_root_without_touching_it() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let missing_root = temp.path().join("does-not-exist");
        let board = Board {
            name: "rpi4".to_string(),
            config: BoardConfig {
                model: "rpi4".to_string(),
                preset: "rpi4-aarch64-debug".to_string(),
                build_target: "rpi-artifacts".to_string(),
                transport: Transport::NativeTftp,
                artifact_dir: None,
                dtb_path: None,
                core_kobj_dir: None,
                tftp_root: Some(missing_root.clone()),
                tftp_prefix: None,
                serial_device: None,
                serial_baud: 115_200,
                debug_transport: None,
                power_control: None,
                network: None,
                usb_ecm: None,
            },
            config_path: temp.path().join("boards.toml"),
        };

        let report = inspect(&board, temp.path());
        assert!(report.has_failures());
        assert!(report
            .checks
            .iter()
            .any(|check| { check.label == "tftp root" && check.status == CheckStatus::Failure }));
        assert!(!missing_root.exists());
    }
}
