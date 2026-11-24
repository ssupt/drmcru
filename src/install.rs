use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const MKINITCPIO_DROPIN: &str = "/etc/mkinitcpio.conf.d/20-drmcru.conf";
const MAX_EDID_BYTES: u64 = 256 * 128;

/// Describes everything needed to install a custom EDID into the system.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    /// DRM connector name, e.g. "DP-1"
    pub connector: String,
    /// Path to the exported .bin file (source)
    pub edid_source: PathBuf,
    /// Filename inside /lib/firmware/edid/
    pub edid_file_name: String,
    /// The kernel parameter value, e.g. "drm.edid_firmware=DP-1:edid/custom.bin"
    pub kernel_parameter: String,
}

impl InstallPlan {
    pub fn firmware_target(&self) -> PathBuf {
        PathBuf::from(format!("/lib/firmware/edid/{}", self.edid_file_name))
    }
}

/// Describes a previously-installed drmcru EDID override to remove.
#[derive(Debug, Clone)]
pub struct UninstallPlan {
    /// DRM connector name, e.g. "DP-1"
    pub connector: String,
    /// Filename inside /lib/firmware/edid/
    pub edid_file_name: String,
    /// The kernel parameter value to remove.
    pub kernel_parameter: String,
}

impl UninstallPlan {
    pub fn firmware_target(&self) -> PathBuf {
        PathBuf::from(format!("/lib/firmware/edid/{}", self.edid_file_name))
    }
}

/// What the persistent EDID install will modify on the system.
#[derive(Debug, Clone)]
pub struct InstallPreview {
    pub connector: String,
    pub edid_source: String,
    pub firmware_target: String,
    pub mkinitcpio_dropin: String,
    pub bootloader_conf: String,
    pub limine_entry_tool_dropin: String,
    pub kernel_parameter: String,
}

#[derive(Debug, Clone)]
pub struct UninstallPreview {
    pub connector: String,
    pub firmware_target: String,
    pub mkinitcpio_dropin: String,
    pub bootloader_conf: String,
    pub limine_entry_tool_dropin: String,
    pub kernel_parameter: String,
}

/// Non-privileged view of whether drmcru's EDID override appears installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledOverrideStatus {
    pub connector: String,
    pub edid_file_name: String,
    pub kernel_parameter: String,
    pub firmware_target: PathBuf,
    pub firmware_present: bool,
    pub mkinitcpio_references_firmware: bool,
    pub bootloader_references_kernel_parameter: bool,
    pub bootloader_validation_deferred: bool,
    pub limine_entry_tool_references_kernel_parameter: bool,
    pub bootloader_cmdline_entries: usize,
    pub bootloader_cmdline_entries_with_kernel_parameter: usize,
    pub active_kernel_references_kernel_parameter: bool,
    pub read_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSupportReport {
    pub pkexec_available: bool,
    pub mkinitcpio_available: bool,
    pub mkinitcpio_conf_present: bool,
    pub mkinitcpio_presets_present: bool,
    pub limine_mkinitcpio_available: bool,
    pub limine_conf_present: bool,
    pub limine_has_cmdline: bool,
    /// The desktop user cannot read the Limine config, so the root installer
    /// must validate it after authentication.
    pub limine_validation_deferred: bool,
    pub read_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirmwareEdidComparison {
    Matches {
        bytes: usize,
    },
    Differs {
        firmware_bytes: usize,
        live_bytes: usize,
    },
    MissingFirmware,
    MissingLiveEdid,
    ReadError(String),
}

impl FirmwareEdidComparison {
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Matches { .. } => "matches",
            Self::Differs { .. } => "differs",
            Self::MissingFirmware => "missing firmware",
            Self::MissingLiveEdid => "no live EDID",
            Self::ReadError(_) => "read error",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Matches { bytes } => {
                format!("installed firmware matches live EDID ({bytes} bytes)")
            }
            Self::Differs {
                firmware_bytes,
                live_bytes,
            } => format!(
                "installed firmware differs from live EDID ({firmware_bytes} vs {live_bytes} bytes)"
            ),
            Self::MissingFirmware => "installed firmware file is missing".to_string(),
            Self::MissingLiveEdid => "selected connector has no readable live EDID".to_string(),
            Self::ReadError(error) => format!("could not read installed firmware: {error}"),
        }
    }
}

impl SystemSupportReport {
    pub fn is_supported(&self) -> bool {
        self.blocking_issues().is_empty()
    }

    pub fn has_rebuild_backend(&self) -> bool {
        self.limine_mkinitcpio_available
    }

    pub fn rebuild_backend_label(&self) -> &'static str {
        if self.limine_mkinitcpio_available {
            "verified per-kernel mkinitcpio builds"
        } else {
            "missing"
        }
    }

    pub fn blocking_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !self.pkexec_available {
            issues.push("pkexec was not found in PATH".to_string());
        }
        if !self.mkinitcpio_available {
            issues.push("mkinitcpio was not found in PATH".to_string());
        }
        if !self.mkinitcpio_conf_present {
            issues.push("/etc/mkinitcpio.conf was not found".to_string());
        }
        if !self.has_rebuild_backend() {
            issues.push(
                "verified rebuilds require the Limine/mkinitcpio integration; use Export for custom preset-only systems"
                    .to_string(),
            );
        }
        if !self.limine_conf_present {
            issues.push("/boot/limine.conf was not found".to_string());
        } else if !self.limine_validation_deferred && !self.limine_has_cmdline {
            issues.push("/boot/limine.conf has no cmdline: entries".to_string());
        }
        issues.extend(self.read_warnings.iter().cloned());
        issues
    }

    pub fn summary_lines(&self) -> Vec<String> {
        vec![
            format!("Preflight: pkexec {}", yes_no(self.pkexec_available)),
            format!(
                "Preflight: mkinitcpio {}",
                yes_no(self.mkinitcpio_available)
            ),
            format!(
                "Preflight: /etc/mkinitcpio.conf {}",
                yes_no(self.mkinitcpio_conf_present)
            ),
            format!(
                "Preflight: mkinitcpio presets {}",
                yes_no(self.mkinitcpio_presets_present)
            ),
            format!(
                "Preflight: limine-mkinitcpio {}",
                yes_no(self.limine_mkinitcpio_available)
            ),
            format!(
                "Preflight: rebuild backend {}",
                self.rebuild_backend_label()
            ),
            if self.limine_validation_deferred {
                "Preflight: /boot/limine.conf deferred (will verify after authentication)"
                    .to_string()
            } else {
                format!(
                    "Preflight: /boot/limine.conf {}",
                    yes_no(self.limine_conf_present)
                )
            },
            if self.limine_validation_deferred {
                "Preflight: Limine cmdline entries deferred".to_string()
            } else {
                format!(
                    "Preflight: Limine cmdline entries {}",
                    yes_no(self.limine_has_cmdline)
                )
            },
        ]
    }

    pub fn report_text(&self) -> String {
        if self.is_supported() {
            self.summary_lines().join("\n")
        } else {
            std::iter::once(
                "Automatic Apply requires Limine entry-tool and its mkinitcpio integration; custom preset-only systems should use Export.".to_string(),
            )
            .chain(
                self.blocking_issues()
                    .into_iter()
                    .map(|issue| format!("- {issue}")),
            )
            .collect::<Vec<_>>()
            .join("\n")
        }
    }
}

impl InstalledOverrideStatus {
    pub fn has_any_override(&self) -> bool {
        self.firmware_present
            || self.mkinitcpio_references_firmware
            || self.bootloader_references_kernel_parameter
            || self.limine_entry_tool_references_kernel_parameter
            || self.active_kernel_references_kernel_parameter
    }

    pub fn is_active(&self) -> bool {
        self.firmware_present && self.active_kernel_references_kernel_parameter
    }

    pub fn is_configured_for_next_boot(&self) -> bool {
        self.firmware_present
            && if self.bootloader_validation_deferred {
                self.mkinitcpio_references_firmware
                    && self.limine_entry_tool_references_kernel_parameter
            } else {
                self.bootloader_cmdline_entries > 0
                    && self.bootloader_cmdline_entries
                        == self.bootloader_cmdline_entries_with_kernel_parameter
            }
    }

    pub fn short_label(&self) -> &'static str {
        if self.is_active() {
            "active"
        } else if self.is_configured_for_next_boot() {
            "installed, reboot pending"
        } else if self.has_any_override() {
            "partial install"
        } else {
            "not installed"
        }
    }
}

impl UninstallPreview {
    pub fn from_plan(plan: &UninstallPlan) -> Self {
        Self {
            connector: plan.connector.clone(),
            firmware_target: plan.firmware_target().display().to_string(),
            mkinitcpio_dropin: MKINITCPIO_DROPIN.to_string(),
            bootloader_conf: "/boot/limine.conf".to_string(),
            limine_entry_tool_dropin: "/etc/limine-entry-tool.d/drmcru-edid.conf".to_string(),
            kernel_parameter: plan.kernel_parameter.clone(),
        }
    }

    pub fn summary_lines(&self) -> Vec<String> {
        vec![
            format!("Connector:  {}", self.connector),
            format!("Remove:     {}", self.firmware_target),
            format!(
                "Patch:      {} (remove EDID from FILES)",
                self.mkinitcpio_dropin
            ),
            "Clean:      /etc/mkinitcpio.conf (remove any legacy EDID entry)".to_string(),
            format!(
                "Patch:      {} (remove kernel param from cmdline entries)",
                self.bootloader_conf
            ),
            format!(
                "Patch:      {} (remove persistent Limine entry-tool param)",
                self.limine_entry_tool_dropin
            ),
            "Patch:      /etc/default/limine (managed effective kernel command lines)".to_string(),
            format!("Parameter:  {}", self.kernel_parameter),
            "Run:        Build and verify kernel images before updating boot entries".to_string(),
            "Recovery:   /boot/drmcru-recovery (preserve the previous boot entry)".to_string(),
        ]
    }
}

impl InstallPreview {
    pub fn from_plan(plan: &InstallPlan) -> Self {
        Self {
            connector: plan.connector.clone(),
            edid_source: plan.edid_source.display().to_string(),
            firmware_target: plan.firmware_target().display().to_string(),
            mkinitcpio_dropin: MKINITCPIO_DROPIN.to_string(),
            bootloader_conf: "/boot/limine.conf".to_string(),
            limine_entry_tool_dropin: "/etc/limine-entry-tool.d/drmcru-edid.conf".to_string(),
            kernel_parameter: plan.kernel_parameter.clone(),
        }
    }

    pub fn summary_lines(&self) -> Vec<String> {
        vec![
            format!("Connector:  {}", self.connector),
            format!(
                "Copy EDID:  {} → {}",
                self.edid_source, self.firmware_target
            ),
            format!(
                "Patch:      {} (append EDID to FILES)",
                self.mkinitcpio_dropin
            ),
            "Migrate:    /etc/mkinitcpio.conf (remove any legacy EDID entry)".to_string(),
            format!(
                "Patch:      {} (add kernel param to cmdline entries)",
                self.bootloader_conf
            ),
            format!(
                "Patch:      {} (add persistent Limine entry-tool param)",
                self.limine_entry_tool_dropin
            ),
            "Patch:      /etc/default/limine (managed effective kernel command lines)".to_string(),
            format!("Parameter:  {}", self.kernel_parameter),
            "Run:        Build and verify kernel images before updating boot entries".to_string(),
            "Recovery:   /boot/drmcru-recovery (preserve the previous boot entry)".to_string(),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub success: bool,
    pub output: String,
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("EDID source file does not exist or is invalid: {0}")]
    MissingEdid(PathBuf),
    #[error("failed to read or stage EDID file: {0}")]
    EdidIo(io::Error),
    #[error("unsafe firmware file name: {0}")]
    UnsafeFirmwareName(String),
    #[error("failed to write privileged script: {0}")]
    ScriptWrite(io::Error),
    #[error("pkexec was not found; install polkit to use EDID install/uninstall")]
    NoPkexec,
    #[error("failed to launch pkexec: {0}")]
    Launch(io::Error),
    #[error("privileged script failed (exit {code}): {output}")]
    ScriptFailed { code: i32, output: String },
    #[error("privileged script was cancelled by the user")]
    Cancelled,
    #[error(
        "authorization failed; check the password and that a polkit authentication agent is running: {0}"
    )]
    NotAuthorized(String),
    #[error("unsupported automatic Apply environment: {0}")]
    UnsupportedSystem(String),
}

pub fn inspect_system_support() -> SystemSupportReport {
    let mut report = inspect_system_support_with_paths(
        Path::new("/etc/mkinitcpio.conf"),
        Path::new("/etc/mkinitcpio.d"),
        Path::new("/boot/limine.conf"),
        command_exists("pkexec"),
        command_exists("mkinitcpio"),
        command_exists("limine-mkinitcpio"),
    );
    for command in [
        "limine-entry-tool",
        "lsinitcpio",
        "objcopy",
        "pacman",
        "flock",
        "b2sum",
        "sha256sum",
        "realpath",
        "diff",
        "findmnt",
    ] {
        if !command_exists(command) {
            report.read_warnings.push(format!("{command} is required for verified automatic installation; manual Export remains available"));
        }
    }
    if !Path::new("/usr/lib/limine/limine-common-functions").is_file() {
        report
            .read_warnings
            .push("Limine entry-tool integration was not found".into());
    }
    if !report.limine_mkinitcpio_available {
        report.read_warnings.push("Automatic installation requires the Limine/mkinitcpio integration; use Export for custom preset-only setups".into());
    }
    report
}

fn inspect_system_support_with_paths(
    mkinitcpio_conf: &Path,
    mkinitcpio_dir: &Path,
    limine_conf: &Path,
    pkexec_available: bool,
    mkinitcpio_available: bool,
    limine_mkinitcpio_available: bool,
) -> SystemSupportReport {
    let mut read_warnings = Vec::new();
    let mkinitcpio_conf_present =
        path_exists(mkinitcpio_conf, "mkinitcpio config", &mut read_warnings);
    let mkinitcpio_presets_present = has_mkinitcpio_presets(mkinitcpio_dir, &mut read_warnings);
    if !limine_mkinitcpio_available && mkinitcpio_presets_present {
        inspect_preset_dropin_support(mkinitcpio_dir, &mut read_warnings);
    }
    let (limine_conf_present, limine_has_cmdline, limine_validation_deferred) =
        inspect_limine_config(limine_conf, &mut read_warnings);

    SystemSupportReport {
        pkexec_available,
        mkinitcpio_available,
        mkinitcpio_conf_present,
        mkinitcpio_presets_present,
        limine_mkinitcpio_available,
        limine_conf_present,
        limine_has_cmdline,
        limine_validation_deferred,
        read_warnings,
    }
}

fn inspect_limine_config(path: &Path, warnings: &mut Vec<String>) -> (bool, bool, bool) {
    inspect_limine_config_read(fs::read_to_string(path), path, warnings)
}

fn inspect_limine_config_read(
    result: io::Result<String>,
    path: &Path,
    warnings: &mut Vec<String>,
) -> (bool, bool, bool) {
    match result {
        Ok(contents) => (
            true,
            contents
                .lines()
                .any(|line| line.trim_start().starts_with("cmdline:")),
            false,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (false, false, false),
        // The privileged installer performs the same validation before making
        // changes. Do not prevent it from reaching pkexec just because /boot
        // is intentionally unreadable to the desktop user.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => (true, false, true),
        Err(error) => {
            warnings.push(format!(
                "Could not read Limine config {}: {error}",
                path.display()
            ));
            (false, false, false)
        }
    }
}

pub fn inspect_installed_override(plan: &UninstallPlan) -> InstalledOverrideStatus {
    inspect_override_with_paths(
        &plan.connector,
        &plan.edid_file_name,
        &plan.kernel_parameter,
        OverrideInspectionPaths {
            firmware_target: &plan.firmware_target(),
            mkinitcpio_conf: Path::new("/etc/mkinitcpio.conf"),
            mkinitcpio_dropin: Path::new(MKINITCPIO_DROPIN),
            bootloader_conf: Path::new("/boot/limine.conf"),
            limine_entry_tool_dropin: Path::new("/etc/limine-entry-tool.d/drmcru-edid.conf"),
            active_cmdline: Path::new("/proc/cmdline"),
        },
    )
}

pub fn compare_firmware_to_live_edid(
    firmware_target: &Path,
    live_edid: Option<&[u8]>,
) -> FirmwareEdidComparison {
    let Some(live_edid) = live_edid else {
        return FirmwareEdidComparison::MissingLiveEdid;
    };

    let firmware = match fs::read(firmware_target) {
        Ok(firmware) => firmware,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return FirmwareEdidComparison::MissingFirmware;
        }
        Err(error) => return FirmwareEdidComparison::ReadError(error.to_string()),
    };

    if firmware == live_edid {
        FirmwareEdidComparison::Matches {
            bytes: firmware.len(),
        }
    } else {
        FirmwareEdidComparison::Differs {
            firmware_bytes: firmware.len(),
            live_bytes: live_edid.len(),
        }
    }
}

struct OverrideInspectionPaths<'a> {
    firmware_target: &'a Path,
    mkinitcpio_conf: &'a Path,
    mkinitcpio_dropin: &'a Path,
    bootloader_conf: &'a Path,
    limine_entry_tool_dropin: &'a Path,
    active_cmdline: &'a Path,
}

fn inspect_override_with_paths(
    connector: &str,
    edid_file_name: &str,
    kernel_parameter: &str,
    paths: OverrideInspectionPaths<'_>,
) -> InstalledOverrideStatus {
    let mut read_warnings = Vec::new();
    let firmware_present =
        path_exists(paths.firmware_target, "firmware target", &mut read_warnings);
    let legacy_mkinitcpio_reference = file_references_mkinitcpio_firmware(
        paths.mkinitcpio_conf,
        &paths.firmware_target.display().to_string(),
        "mkinitcpio config",
        &mut read_warnings,
    );
    let dropin_mkinitcpio_reference = file_references_mkinitcpio_firmware(
        paths.mkinitcpio_dropin,
        &paths.firmware_target.display().to_string(),
        "mkinitcpio drop-in",
        &mut read_warnings,
    );
    let mkinitcpio_references_firmware = legacy_mkinitcpio_reference || dropin_mkinitcpio_reference;
    let (
        bootloader_cmdline_entries,
        bootloader_cmdline_entries_with_kernel_parameter,
        bootloader_references_kernel_parameter,
        bootloader_validation_deferred,
    ) = limine_cmdline_reference_stats(
        paths.bootloader_conf,
        kernel_parameter,
        "bootloader config",
        &mut read_warnings,
    );
    let limine_entry_tool_references_kernel_parameter = file_contains_kernel_mapping(
        paths.limine_entry_tool_dropin,
        kernel_parameter,
        "Limine entry-tool drop-in",
        &mut read_warnings,
    );
    let active_kernel_references_kernel_parameter = file_contains_kernel_mapping(
        paths.active_cmdline,
        kernel_parameter,
        "active kernel command line",
        &mut read_warnings,
    );

    InstalledOverrideStatus {
        connector: connector.to_string(),
        edid_file_name: edid_file_name.to_string(),
        kernel_parameter: kernel_parameter.to_string(),
        firmware_target: paths.firmware_target.to_path_buf(),
        firmware_present,
        mkinitcpio_references_firmware,
        bootloader_references_kernel_parameter,
        bootloader_validation_deferred,
        limine_entry_tool_references_kernel_parameter,
        bootloader_cmdline_entries,
        bootloader_cmdline_entries_with_kernel_parameter,
        active_kernel_references_kernel_parameter,
        read_warnings,
    }
}

fn path_exists(path: &Path, label: &str, warnings: &mut Vec<String>) -> bool {
    match fs::metadata(path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            warnings.push(format!(
                "Could not inspect {label} {}: {error}",
                path.display()
            ));
            false
        }
    }
}

fn file_references_mkinitcpio_firmware(
    path: &Path,
    target: &str,
    label: &str,
    warnings: &mut Vec<String>,
) -> bool {
    match fs::read_to_string(path) {
        Ok(contents) => text_references_mkinitcpio_firmware(&contents, target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            warnings.push(format!(
                "Could not read {label} {}: {error}",
                path.display()
            ));
            false
        }
    }
}

fn text_references_mkinitcpio_firmware(contents: &str, target: &str) -> bool {
    contents.lines().any(|line| {
        let uncommented = line.split('#').next().unwrap_or_default();
        uncommented
            .split(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | '(' | ')' | ';')
            })
            .any(|token| token == target)
    })
}

fn file_contains_kernel_mapping(
    path: &Path,
    kernel_parameter: &str,
    label: &str,
    warnings: &mut Vec<String>,
) -> bool {
    match fs::read_to_string(path) {
        Ok(contents) => text_contains_kernel_mapping(&contents, kernel_parameter),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            warnings.push(format!(
                "Could not read {label} {}: {error}",
                path.display()
            ));
            false
        }
    }
}

fn text_contains_kernel_mapping(contents: &str, kernel_parameter: &str) -> bool {
    let Some(expected) = kernel_parameter.strip_prefix("drm.edid_firmware=") else {
        return false;
    };

    contents
        .split(|character: char| character.is_whitespace() || matches!(character, '"' | '\'' | ';'))
        .filter_map(|token| token.strip_prefix("drm.edid_firmware="))
        .flat_map(|value| value.split(','))
        .any(|mapping| mapping == expected)
}

fn limine_cmdline_reference_stats(
    path: &Path,
    needle: &str,
    label: &str,
    warnings: &mut Vec<String>,
) -> (usize, usize, bool, bool) {
    limine_cmdline_reference_stats_read(fs::read_to_string(path), path, needle, label, warnings)
}

fn limine_cmdline_reference_stats_read(
    result: io::Result<String>,
    path: &Path,
    needle: &str,
    label: &str,
    warnings: &mut Vec<String>,
) -> (usize, usize, bool, bool) {
    match result {
        Ok(contents) => {
            let mut cmdlines = 0;
            let mut matching = 0;
            let mut recovery = false;
            for line in contents.lines() {
                if line.trim() == "# BEGIN drmcru recovery" {
                    recovery = true;
                }
                if line.trim() == "# END drmcru recovery" {
                    recovery = false;
                    continue;
                }
                if recovery {
                    continue;
                }
                if line.trim_start().starts_with("cmdline:") {
                    cmdlines += 1;
                    if text_contains_kernel_mapping(line, needle) {
                        matching += 1;
                    }
                }
            }
            (cmdlines, matching, matching > 0, false)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (0, 0, false, false),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => (0, 0, false, true),
        Err(error) => {
            warnings.push(format!(
                "Could not read {label} {}: {error}",
                path.display()
            ));
            (0, 0, false, false)
        }
    }
}

fn has_mkinitcpio_presets(path: &Path, warnings: &mut Vec<String>) -> bool {
    match fs::read_dir(path) {
        Ok(entries) => entries.filter_map(Result::ok).any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "preset")
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            warnings.push(format!(
                "Could not inspect mkinitcpio preset directory {}: {error}",
                path.display()
            ));
            false
        }
    }
}

fn inspect_preset_dropin_support(path: &Path, warnings: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(path) else {
        return; // The preset-directory check already reports read errors.
    };
    for entry in entries {
        let preset = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                warnings.push(format!("Could not inspect mkinitcpio preset: {error}"));
                continue;
            }
        };
        if preset
            .extension()
            .is_none_or(|extension| extension != "preset")
        {
            continue;
        }
        match Command::new("awk")
            .arg(include_str!("mkinitcpio_preset_config.awk"))
            .arg(&preset)
            .output()
        {
            Ok(output) if output.status.success() => {}
            Ok(output) => warnings.push(format!(
                "Could not use mkinitcpio drop-ins with {}: {}{}",
                preset.display(),
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => warnings.push(format!(
                "Could not check mkinitcpio preset {}: {error}",
                preset.display()
            )),
        }
    }
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return is_executable_file(path);
    }

    std::env::var_os("PATH").as_deref().is_some_and(|path| {
        std::env::split_paths(path).any(|dir| is_executable_file(&dir.join(command)))
    })
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn yes_no(value: bool) -> &'static str {
    if value { "ok" } else { "missing" }
}

/// Generate the shared, verified boot transaction for either operation.
#[cfg(test)]
pub fn build_install_script(plan: &InstallPlan) -> String {
    let edid_sha256 = read_install_edid(&plan.edid_source)
        .ok()
        .map(|bytes| sha256_hex(&bytes));
    build_override_script(
        &plan.connector,
        &plan.edid_file_name,
        Some(&plan.edid_source),
        edid_sha256.as_deref(),
    )
}

fn build_install_script_with_sha256(plan: &InstallPlan, edid_sha256: &str) -> String {
    build_override_script(
        &plan.connector,
        &plan.edid_file_name,
        Some(&plan.edid_source),
        Some(edid_sha256),
    )
}

pub fn build_uninstall_script(plan: &UninstallPlan) -> String {
    build_override_script(&plan.connector, &plan.edid_file_name, None, None)
}

fn build_override_script(
    connector: &str,
    name: &str,
    source: Option<&Path>,
    edid_sha256: Option<&str>,
) -> String {
    let connector = shell_quote(connector);
    let name = shell_quote(name);
    let operation = if source.is_some() { "add" } else { "remove" };
    let source = shell_quote(
        &source
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    );
    let edid_sha256 = shell_quote(edid_sha256.unwrap_or_default());
    let files_awk = shell_quote(include_str!("mkinitcpio_files.awk"));
    let cmdline_awk = shell_quote(include_str!("kernel_cmdline.awk"));
    let entry_awk = shell_quote(include_str!("limine_entry.awk"));
    let mkinit_helpers = include_str!("mkinitcpio_dropin.sh");
    let transaction = include_str!("boot_transaction.sh");
    format!(
        r##"#!/bin/bash
OPERATION={operation}
CONNECTOR={connector}
EDID_NAME={name}
EDID_SOURCE={source}
EDID_SHA256={edid_sha256}
MKINITCPIO_BIN="/usr/bin/mkinitcpio"
LSINITCPIO_BIN="/usr/bin/lsinitcpio"
ENTRY_TOOL_BIN="/usr/bin/limine-entry-tool"
OBJCOPY_BIN="/usr/bin/objcopy"
FIRMWARE_ROOT="/lib/firmware"
FIRMWARE_TARGET="$FIRMWARE_ROOT/edid/$EDID_NAME"
BOOT_ROOT="/boot"
LIMINE="$BOOT_ROOT/limine.conf"
MKINIT="/etc/mkinitcpio.conf"
MKINIT_DROPIN_DIR="$MKINIT.d"
MKINIT_DROPIN="$MKINIT_DROPIN_DIR/20-drmcru.conf"
LIMINE_DROPIN_DIR="/etc/limine-entry-tool.d"
LIMINE_DROPIN="$LIMINE_DROPIN_DIR/drmcru-edid.conf"
LIMINE_CONFIG="/etc/limine-entry-tool.conf"
LIMINE_DEFAULT_CONFIG="/etc/default/limine"
LIMINE_VENDOR_DIR="/usr/share/limine-entry-tool.d"
LIMINE_FUNCTIONS="/usr/lib/limine/limine-common-functions"
BOOT_HOOKS="/etc/boot/hooks"
BOOT_LOCK="/run/lock/boot-partition.lock"
PACKAGE_LOCK="/var/lib/pacman/db.lck"
TRANSACTION_ROOT="/var/tmp"
MODULES_ROOT="/usr/lib/modules"
MACHINE_ID_FILE="/etc/machine-id"
DRM_ROOT="/sys/class/drm"
FILES_AWK={files_awk}
CMDLINE_AWK={cmdline_awk}
ENTRY_AWK={entry_awk}
{mkinit_helpers}
{transaction}
"##
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn validate_system_connector_mapping(connector: &str) -> Result<(), String> {
    let names = fs::read_dir("/sys/class/drm")
        .map_err(|error| format!("Could not inspect DRM connectors: {error}"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    let connectors = names
        .iter()
        .filter(|path| path.join("status").exists())
        .filter_map(|path| {
            path.file_name()?
                .to_str()?
                .split_once('-')
                .map(|(_, name)| name)
        });
    validate_connector_mapping(connector, connectors)
}

pub fn stage_temp_edid(bytes: &[u8]) -> Result<PathBuf, InstallError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let mut last_error = None;

    for attempt in 0..100 {
        let path = std::env::temp_dir().join(format!("drmcru-edid-{pid}-{now}-{attempt}.bin"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes) {
                    let _ = fs::remove_file(&path);
                    return Err(InstallError::EdidIo(error));
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => return Err(InstallError::EdidIo(error)),
        }
    }

    Err(InstallError::EdidIo(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create unique staged EDID file",
        )
    })))
}

fn read_install_edid(path: &Path) -> io::Result<Vec<u8>> {
    let link_meta = fs::symlink_metadata(path)?;
    if link_meta.file_type().is_symlink() || !link_meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "EDID source must be a regular file, not a symlink or special file",
        ));
    }
    if link_meta.len() == 0 || link_meta.len() > MAX_EDID_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("EDID source must be 1..={MAX_EDID_BYTES} bytes"),
        ));
    }
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_EDID_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "EDID source changed to an invalid file",
        ));
    }
    let mut raw = Vec::new();
    file.take(MAX_EDID_BYTES + 1).read_to_end(&mut raw)?;
    if raw.is_empty() || raw.len() as u64 > MAX_EDID_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "EDID source size is invalid",
        ));
    }
    Ok(raw)
}

/// Run the install. Returns a report on success, or an error.
pub fn install(plan: &InstallPlan) -> Result<InstallReport, InstallError> {
    validate_firmware_file_name(&plan.edid_file_name)?;
    validate_system_connector_mapping(&plan.connector).map_err(InstallError::UnsupportedSystem)?;

    let edid_bytes = read_install_edid(&plan.edid_source).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            InstallError::MissingEdid(plan.edid_source.clone())
        } else {
            InstallError::EdidIo(error)
        }
    })?;
    let edid_sha256 = sha256_hex(&edid_bytes);

    let support = inspect_system_support();
    if !support.is_supported() {
        return Err(InstallError::UnsupportedSystem(support.report_text()));
    }

    run_privileged_script(&build_install_script_with_sha256(plan, &edid_sha256))
}

/// Run the uninstall. Returns a report on success, or an error.
pub fn uninstall(plan: &UninstallPlan) -> Result<InstallReport, InstallError> {
    validate_firmware_file_name(&plan.edid_file_name)?;
    let support = inspect_system_support();
    if !support.is_supported() {
        return Err(InstallError::UnsupportedSystem(support.report_text()));
    }
    run_privileged_script(&build_uninstall_script(plan))
}

fn run_privileged_script(script: &str) -> Result<InstallReport, InstallError> {
    if Command::new("pkexec")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        return Err(InstallError::NoPkexec);
    }

    let script_path = write_temp_install_script(script)?;

    // Run via pkexec
    let result = Command::new("pkexec")
        .arg("bash")
        .arg(&script_path)
        .output();

    // Clean up even when pkexec cannot be launched.
    let _ = std::fs::remove_file(&script_path);
    let result = result.map_err(InstallError::Launch)?;

    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    let output = if stderr.is_empty() {
        stdout
    } else {
        format!("{stdout}\n--- stderr ---\n{stderr}")
    };

    if result.status.success() {
        Ok(InstallReport {
            success: true,
            output,
        })
    } else {
        Err(classify_failure(result.status.code().unwrap_or(-1), output))
    }
}

/// pkexec exits 126 when the dialog is dismissed and 127 when authorization
/// fails; the privileged script maps those codes to 1 so they stay unambiguous.
fn classify_failure(code: i32, output: String) -> InstallError {
    match code {
        126 => InstallError::Cancelled,
        127 => InstallError::NotAuthorized(output),
        _ => InstallError::ScriptFailed { code, output },
    }
}

fn validate_firmware_file_name(edid_file_name: &str) -> Result<(), InstallError> {
    if edid_file_name.is_empty()
        || matches!(edid_file_name, "." | "..")
        || !edid_file_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(InstallError::UnsafeFirmwareName(edid_file_name.to_string()));
    }

    Ok(())
}

pub(crate) fn validate_connector_mapping<'a>(
    connector: &str,
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut exact = 0;
    for name in names {
        if name == connector {
            exact += 1;
        } else if name.starts_with(connector) {
            return Err(format!(
                "Linux also applies connector mapping {connector} to {name}; use an unambiguous connector"
            ));
        }
    }
    if exact != 1 {
        return Err(format!(
            "Connector {connector} is absent or occurs on multiple DRM devices"
        ));
    }
    Ok(())
}

fn write_temp_install_script(script: &str) -> Result<PathBuf, InstallError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let mut last_error = None;

    for attempt in 0..100 {
        let path = std::env::temp_dir().join(format!("drmcru-install-{pid}-{now}-{attempt}.sh"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(script.as_bytes()) {
                    let _ = fs::remove_file(&path);
                    return Err(InstallError::ScriptWrite(error));
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => return Err(InstallError::ScriptWrite(error)),
        }
    }

    Err(InstallError::ScriptWrite(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create unique temporary install script",
        )
    })))
}

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (index, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            w[index] = u32::from_be_bytes(*chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, delta) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(delta);
        }
    }

    let mut hex = String::with_capacity(64);
    for word in state {
        use std::fmt::Write as _;
        let _ = write!(hex, "{word:08x}");
    }
    hex
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
