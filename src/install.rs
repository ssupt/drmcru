use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

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
    pub mkinitcpio_conf: String,
    pub bootloader_conf: String,
    pub limine_entry_tool_dropin: String,
    pub kernel_parameter: String,
}

#[derive(Debug, Clone)]
pub struct UninstallPreview {
    pub connector: String,
    pub firmware_target: String,
    pub mkinitcpio_conf: String,
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
        self.limine_mkinitcpio_available || self.mkinitcpio_presets_present
    }

    pub fn rebuild_backend_label(&self) -> &'static str {
        if self.limine_mkinitcpio_available {
            "limine-mkinitcpio"
        } else if self.mkinitcpio_presets_present {
            "mkinitcpio -P"
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
                "no initramfs rebuild backend found (expected limine-mkinitcpio or /etc/mkinitcpio.d/*.preset)"
                    .to_string(),
            );
        }
        if !self.limine_conf_present {
            issues.push("/boot/limine.conf was not found".to_string());
        } else if !self.limine_has_cmdline {
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
            format!(
                "Preflight: /boot/limine.conf {}",
                yes_no(self.limine_conf_present)
            ),
            format!(
                "Preflight: Limine cmdline entries {}",
                yes_no(self.limine_has_cmdline)
            ),
        ]
    }

    pub fn report_text(&self) -> String {
        if self.is_supported() {
            self.summary_lines().join("\n")
        } else {
            std::iter::once(
                "Automatic Apply currently supports Limine systems with mkinitcpio presets or limine-mkinitcpio.".to_string(),
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
            && self.bootloader_cmdline_entries > 0
            && self.bootloader_cmdline_entries
                == self.bootloader_cmdline_entries_with_kernel_parameter
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
            mkinitcpio_conf: "/etc/mkinitcpio.conf".to_string(),
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
                self.mkinitcpio_conf
            ),
            format!(
                "Patch:      {} (remove kernel param from cmdline entries)",
                self.bootloader_conf
            ),
            format!(
                "Patch:      {} (remove persistent Limine entry-tool param)",
                self.limine_entry_tool_dropin
            ),
            format!("Parameter:  {}", self.kernel_parameter),
            "Run:        limine-mkinitcpio when present, otherwise mkinitcpio -P".to_string(),
        ]
    }
}

impl InstallPreview {
    pub fn from_plan(plan: &InstallPlan) -> Self {
        Self {
            connector: plan.connector.clone(),
            edid_source: plan.edid_source.display().to_string(),
            firmware_target: plan.firmware_target().display().to_string(),
            mkinitcpio_conf: "/etc/mkinitcpio.conf".to_string(),
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
            format!("Patch:      {} (add EDID to FILES)", self.mkinitcpio_conf),
            format!(
                "Patch:      {} (add kernel param to cmdline entries)",
                self.bootloader_conf
            ),
            format!(
                "Patch:      {} (add persistent Limine entry-tool param)",
                self.limine_entry_tool_dropin
            ),
            format!("Parameter:  {}", self.kernel_parameter),
            "Run:        limine-mkinitcpio when present, otherwise mkinitcpio -P".to_string(),
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
    #[error("EDID source file does not exist: {0}")]
    MissingEdid(PathBuf),
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
    #[error("unsupported automatic Apply environment: {0}")]
    UnsupportedSystem(String),
}

pub fn inspect_system_support() -> SystemSupportReport {
    inspect_system_support_with_paths(
        Path::new("/etc/mkinitcpio.conf"),
        Path::new("/etc/mkinitcpio.d"),
        Path::new("/boot/limine.conf"),
        command_exists("pkexec"),
        command_exists("mkinitcpio"),
        command_exists("limine-mkinitcpio"),
    )
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
    let limine_conf_present = path_exists(limine_conf, "Limine config", &mut read_warnings);
    let limine_has_cmdline = if limine_conf_present {
        match fs::read_to_string(limine_conf) {
            Ok(contents) => contents
                .lines()
                .any(|line| line.trim_start().starts_with("cmdline:")),
            Err(error) => {
                read_warnings.push(format!(
                    "Could not read Limine config {}: {error}",
                    limine_conf.display()
                ));
                false
            }
        }
    } else {
        false
    };

    SystemSupportReport {
        pkexec_available,
        mkinitcpio_available,
        mkinitcpio_conf_present,
        mkinitcpio_presets_present,
        limine_mkinitcpio_available,
        limine_conf_present,
        limine_has_cmdline,
        read_warnings,
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
    let mkinitcpio_references_firmware = file_contains(
        paths.mkinitcpio_conf,
        &paths.firmware_target.display().to_string(),
        "mkinitcpio config",
        &mut read_warnings,
    );
    let (
        bootloader_cmdline_entries,
        bootloader_cmdline_entries_with_kernel_parameter,
        bootloader_references_kernel_parameter,
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

fn file_contains(path: &Path, needle: &str, label: &str, warnings: &mut Vec<String>) -> bool {
    match fs::read_to_string(path) {
        Ok(contents) => contents.contains(needle),
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
) -> (usize, usize, bool) {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let mut cmdlines = 0;
            let mut matching = 0;
            for line in contents.lines() {
                if line.trim_start().starts_with("cmdline:") {
                    cmdlines += 1;
                    if text_contains_kernel_mapping(line, needle) {
                        matching += 1;
                    }
                }
            }
            (cmdlines, matching, matching > 0)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (0, 0, false),
        Err(error) => {
            warnings.push(format!(
                "Could not read {label} {}: {error}",
                path.display()
            ));
            (0, 0, false)
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

/// Build the idempotent bash install script.
pub fn build_install_script(plan: &InstallPlan) -> String {
    let edid_source = shell_quote(&plan.edid_source.display().to_string());
    let firmware_target = shell_quote(&plan.firmware_target().display().to_string());
    let kernel_param = shell_quote(&plan.kernel_parameter);
    let connector = shell_quote(&plan.connector);

    // The script is idempotent and validates the supported stack before mutating files.
    format!(
        r##"#!/bin/bash
set -euo pipefail

echo "=== drmcru: installing custom EDID ==="

if [ "$(id -u)" -ne 0 ]; then
    echo "[ERR] This installer must run as root." >&2
    exit 1
fi

EDID_SOURCE={edid_source}
FIRMWARE_TARGET={firmware_target}
KERNEL_PARAM={kernel_param}
CONNECTOR={connector}
MKINIT="/etc/mkinitcpio.conf"
LIMINE="/boot/limine.conf"
LIMINE_DROPIN_DIR="/etc/limine-entry-tool.d"
LIMINE_DROPIN="$LIMINE_DROPIN_DIR/drmcru-edid.conf"
BACKUP_SUFFIX=".drmcru.$(date +%Y%m%d-%H%M%S-%N)-$$.bak"
declare -a BACKUP_ORIGINALS=()
declare -a BACKUP_PATHS=()
FIRMWARE_WAS_PRESENT=0

backup_file() {{
    local file="$1"
    local backup="${{file}}${{BACKUP_SUFFIX}}"
    cp -a -- "$file" "$backup"
    BACKUP_ORIGINALS+=("$file")
    BACKUP_PATHS+=("$backup")
    echo "[OK] Backup: $backup"
}}

rollback() {{
    local status="$?"
    trap - ERR
    echo "[ERR] Apply failed; restoring files changed by this run." >&2
    local i
    for ((i=${{#BACKUP_ORIGINALS[@]}} - 1; i >= 0; i--)); do
        cp -a -- "${{BACKUP_PATHS[$i]}}" "${{BACKUP_ORIGINALS[$i]}}" || true
    done
    if [ "$FIRMWARE_WAS_PRESENT" -eq 0 ]; then
        rm -f -- "$FIRMWARE_TARGET" || true
    fi
    exit "$status"
}}

if [ ! -f "$EDID_SOURCE" ]; then
    echo "[ERR] EDID source does not exist: $EDID_SOURCE" >&2
    exit 1
fi

if [ ! -f "$MKINIT" ]; then
    echo "[ERR] /etc/mkinitcpio.conf not found — automatic Apply supports mkinitcpio only" >&2
    exit 1
fi

if [ ! -f "$LIMINE" ]; then
    echo "[ERR] /boot/limine.conf not found — automatic Apply supports Limine only" >&2
    exit 1
fi

if ! grep -qE '^[[:space:]]*cmdline:' "$LIMINE"; then
    echo "[ERR] /boot/limine.conf has no cmdline entries" >&2
    exit 1
fi

if ! command -v mkinitcpio >/dev/null 2>&1; then
    echo "[ERR] mkinitcpio was not found" >&2
    exit 1
fi

if ! command -v limine-mkinitcpio >/dev/null 2>&1 && ! compgen -G "/etc/mkinitcpio.d/*.preset" >/dev/null; then
    echo "[ERR] No initramfs rebuild backend found (expected limine-mkinitcpio or mkinitcpio presets)" >&2
    exit 1
fi

if [ -e "$FIRMWARE_TARGET" ]; then
    FIRMWARE_WAS_PRESENT=1
    backup_file "$FIRMWARE_TARGET"
fi
trap rollback ERR

# 1. Copy EDID to firmware directory
install -D -m 0644 -- "$EDID_SOURCE" "$FIRMWARE_TARGET"
echo "[OK] Installed EDID to $FIRMWARE_TARGET"

# 2. Patch /etc/mkinitcpio.conf — add EDID to FILES if not already present
if [ -f "$MKINIT" ]; then
    if grep -qF -- "$FIRMWARE_TARGET" "$MKINIT" 2>/dev/null; then
        echo "[OK] EDID already in mkinitcpio FILES (skipped)"
    else
        backup_file "$MKINIT"
        TMP="$(mktemp /tmp/drmcru-mkinitcpio.XXXXXX)"
        awk -v path="$FIRMWARE_TARGET" '
            BEGIN {{ inserted = 0 }}
            /^[[:space:]]*FILES=\(/ && !inserted {{
                sub(/\(/, "(" path " ")
                inserted = 1
            }}
            {{ print }}
            END {{
                if (!inserted) {{
                    print ""
                    print "FILES=(" path ")"
                }}
            }}
        ' "$MKINIT" > "$TMP"
        cat "$TMP" > "$MKINIT"
        rm -f "$TMP"
        echo "[OK] Added EDID to mkinitcpio FILES"
    fi
else
    echo "[ERR] /etc/mkinitcpio.conf not found — automatic Apply supports mkinitcpio only" >&2
    exit 1
fi

# 3. Patch Limine entry-tool drop-in when available. limine-mkinitcpio uses this
#    to embed the same kernel parameter into regenerated Limine entries/UKIs.
if command -v limine-mkinitcpio >/dev/null 2>&1; then
    mkdir -p "$LIMINE_DROPIN_DIR"
    if [ -f "$LIMINE_DROPIN" ]; then
        backup_file "$LIMINE_DROPIN"
        TMP="$(mktemp /tmp/drmcru-limine-entry.XXXXXX)"
        awk -v connector="$CONNECTOR" -v param="$KERNEL_PARAM" '
            function add_mapping(mapping) {{
                if (mapping == "" || index(mapping, connector ":") == 1)
                    return
                if (!seen[mapping]++)
                    mappings[++mapping_count] = mapping
            }}
            /^[[:space:]]*KERNEL_CMDLINE\[default\]\+=/ && index($0, "drm.edid_firmware=") > 0 {{
                value = substr($0, index($0, "drm.edid_firmware=") + length("drm.edid_firmware="))
                sub(/[\"[:space:]].*$/, "", value)
                count = split(value, values, ",")
                for (i = 1; i <= count; i++)
                    add_mapping(values[i])
                next
            }}
            {{ print }}
            END {{
                desired = substr(param, length("drm.edid_firmware=") + 1)
                add_mapping(desired)
                combined = ""
                for (i = 1; i <= mapping_count; i++)
                    combined = combined (combined == "" ? "" : ",") mappings[i]
                print "KERNEL_CMDLINE[default]+=\" drm.edid_firmware=" combined "\""
            }}
        ' "$LIMINE_DROPIN" > "$TMP"
        cat "$TMP" > "$LIMINE_DROPIN"
        rm -f "$TMP"
    else
        {{
            echo "# Managed by drmcru. Edit through drmcru when possible."
            printf 'KERNEL_CMDLINE[default]+=" %s"\n' "$KERNEL_PARAM"
        }} > "$LIMINE_DROPIN"
    fi
    echo "[OK] Consolidated persistent Limine entry-tool EDID mappings"
else
    echo "[OK] limine-mkinitcpio not present; Limine entry-tool drop-in skipped"
fi

# 4. Patch /boot/limine.conf. The kernel accepts one drm.edid_firmware value;
#    connector mappings inside that value are comma-separated. Consolidate any
#    older repeated parameters while replacing this connector's mapping.
if [ -f "$LIMINE" ]; then
    if grep -qE '^[[:space:]]*cmdline:' "$LIMINE"; then
        backup_file "$LIMINE"
        TMP="$(mktemp /tmp/drmcru-limine.XXXXXX)"
        awk -v connector="$CONNECTOR" -v param="$KERNEL_PARAM" '
            function add_mapping(mapping) {{
                if (mapping == "" || index(mapping, connector ":") == 1)
                    return
                if (!seen[mapping]++)
                    mappings[++mapping_count] = mapping
            }}
            /^[[:space:]]*cmdline:/ {{
                delete seen
                delete mappings
                mapping_count = 0
                prefix = substr($0, 1, index($0, ":"))
                other = ""
                for (i = 2; i <= NF; i++) {{
                    if (index($i, "drm.edid_firmware=") == 1) {{
                        value = substr($i, length("drm.edid_firmware=") + 1)
                        count = split(value, values, ",")
                        for (j = 1; j <= count; j++)
                            add_mapping(values[j])
                    }} else {{
                        other = other (other == "" ? "" : " ") $i
                    }}
                }}
                desired = substr(param, length("drm.edid_firmware=") + 1)
                add_mapping(desired)
                combined = ""
                for (i = 1; i <= mapping_count; i++)
                    combined = combined (combined == "" ? "" : ",") mappings[i]
                print prefix (other == "" ? "" : " " other) " drm.edid_firmware=" combined
                next
            }}
            {{ print }}
        ' "$LIMINE" > "$TMP"
        cat "$TMP" > "$LIMINE"
        rm -f "$TMP"
        echo "[OK] Consolidated EDID mappings in all Limine cmdline entries"
    else
        echo "[ERR] /boot/limine.conf has no cmdline entries" >&2
        exit 1
    fi
else
    echo "[ERR] /boot/limine.conf not found — automatic Apply supports Limine only" >&2
    exit 1
fi

# 5. Rebuild initramfs/UKIs.
if command -v limine-mkinitcpio >/dev/null 2>&1; then
    echo "[..] Rebuilding Limine initramfs/UKI entries (limine-mkinitcpio)..."
    limine-mkinitcpio
    echo "[OK] Limine initramfs/UKI entries rebuilt"
elif compgen -G "/etc/mkinitcpio.d/*.preset" >/dev/null; then
    echo "[..] Rebuilding initramfs (mkinitcpio -P)..."
    mkinitcpio -P
    echo "[OK] Initramfs rebuilt"
else
    echo "[ERR] No initramfs rebuild backend found" >&2
    exit 1
fi

trap - ERR

echo ""
echo "=== Done! Reboot to activate the custom resolution. ==="
"##
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Build the idempotent bash uninstall script.
pub fn build_uninstall_script(plan: &UninstallPlan) -> String {
    let firmware_target = shell_quote(&plan.firmware_target().display().to_string());
    let kernel_param = shell_quote(&plan.kernel_parameter);
    let connector = shell_quote(&plan.connector);

    format!(
        r##"#!/bin/bash
set -euo pipefail

echo "=== drmcru: uninstalling custom EDID ==="

if [ "$(id -u)" -ne 0 ]; then
    echo "[ERR] This uninstaller must run as root." >&2
    exit 1
fi

FIRMWARE_TARGET={firmware_target}
KERNEL_PARAM={kernel_param}
CONNECTOR={connector}
MKINIT="/etc/mkinitcpio.conf"
LIMINE="/boot/limine.conf"
LIMINE_DROPIN_DIR="/etc/limine-entry-tool.d"
LIMINE_DROPIN="$LIMINE_DROPIN_DIR/drmcru-edid.conf"
BACKUP_SUFFIX=".drmcru.$(date +%Y%m%d-%H%M%S-%N)-$$.bak"
declare -a BACKUP_ORIGINALS=()
declare -a BACKUP_PATHS=()
FIRMWARE_WAS_PRESENT=0

backup_file() {{
    local file="$1"
    local backup="${{file}}${{BACKUP_SUFFIX}}"
    cp -a -- "$file" "$backup"
    BACKUP_ORIGINALS+=("$file")
    BACKUP_PATHS+=("$backup")
    echo "[OK] Backup: $backup"
}}

rollback() {{
    local status="$?"
    trap - ERR
    echo "[ERR] Uninstall failed; restoring files changed by this run." >&2
    local i
    for ((i=${{#BACKUP_ORIGINALS[@]}} - 1; i >= 0; i--)); do
        cp -a -- "${{BACKUP_PATHS[$i]}}" "${{BACKUP_ORIGINALS[$i]}}" || true
    done
    exit "$status"
}}

if [ ! -f "$MKINIT" ]; then
    echo "[ERR] /etc/mkinitcpio.conf not found — automatic uninstall supports mkinitcpio only" >&2
    exit 1
fi

if [ ! -f "$LIMINE" ]; then
    echo "[ERR] /boot/limine.conf not found — automatic uninstall supports Limine only" >&2
    exit 1
fi

if ! grep -qE '^[[:space:]]*cmdline:' "$LIMINE"; then
    echo "[ERR] /boot/limine.conf has no cmdline entries" >&2
    exit 1
fi

if ! command -v mkinitcpio >/dev/null 2>&1; then
    echo "[ERR] mkinitcpio was not found" >&2
    exit 1
fi

if ! command -v limine-mkinitcpio >/dev/null 2>&1 && ! compgen -G "/etc/mkinitcpio.d/*.preset" >/dev/null; then
    echo "[ERR] No initramfs rebuild backend found (expected limine-mkinitcpio or mkinitcpio presets)" >&2
    exit 1
fi

if [ -e "$FIRMWARE_TARGET" ]; then
    FIRMWARE_WAS_PRESENT=1
    backup_file "$FIRMWARE_TARGET"
fi
trap rollback ERR

# 1. Remove EDID from firmware directory
if [ -e "$FIRMWARE_TARGET" ]; then
    rm -f -- "$FIRMWARE_TARGET"
    echo "[OK] Removed $FIRMWARE_TARGET"
else
    echo "[OK] Firmware file not present (skipped)"
fi

# 2. Patch /etc/mkinitcpio.conf — remove EDID from FILES if present
if [ -f "$MKINIT" ]; then
    if grep -qF -- "$FIRMWARE_TARGET" "$MKINIT" 2>/dev/null; then
        backup_file "$MKINIT"
        TMP="$(mktemp /tmp/drmcru-mkinitcpio.XXXXXX)"
        awk -v path="$FIRMWARE_TARGET" '
            /^[[:space:]]*FILES=\(/ {{
                while ((pos = index($0, path)) > 0) {{
                    before = substr($0, 1, pos - 1)
                    after = substr($0, pos + length(path))
                    sub(/^[[:space:]]+/, "", after)
                    $0 = before after
                }}
            }}
            {{ print }}
        ' "$MKINIT" > "$TMP"
        cat "$TMP" > "$MKINIT"
        rm -f "$TMP"
        echo "[OK] Removed EDID from mkinitcpio FILES"
    else
        echo "[OK] EDID not present in mkinitcpio FILES (skipped)"
    fi
else
    echo "[ERR] /etc/mkinitcpio.conf not found — automatic uninstall supports mkinitcpio only" >&2
    exit 1
fi

# 3. Patch Limine entry-tool drop-in when present
if [ -f "$LIMINE_DROPIN" ]; then
    if grep -qF -- "drm.edid_firmware=" "$LIMINE_DROPIN" 2>/dev/null; then
        backup_file "$LIMINE_DROPIN"
        TMP="$(mktemp /tmp/drmcru-limine-entry.XXXXXX)"
        awk -v connector="$CONNECTOR" '
            function add_mapping(mapping) {{
                if (mapping == "" || index(mapping, connector ":") == 1)
                    return
                if (!seen[mapping]++)
                    mappings[++mapping_count] = mapping
            }}
            /^[[:space:]]*KERNEL_CMDLINE\[default\]\+=/ && index($0, "drm.edid_firmware=") > 0 {{
                value = substr($0, index($0, "drm.edid_firmware=") + length("drm.edid_firmware="))
                sub(/[\"[:space:]].*$/, "", value)
                count = split(value, values, ",")
                for (i = 1; i <= count; i++)
                    add_mapping(values[i])
                next
            }}
            {{ print }}
            END {{
                if (mapping_count > 0) {{
                    combined = ""
                    for (i = 1; i <= mapping_count; i++)
                        combined = combined (combined == "" ? "" : ",") mappings[i]
                    print "KERNEL_CMDLINE[default]+=\" drm.edid_firmware=" combined "\""
                }}
            }}
        ' "$LIMINE_DROPIN" > "$TMP"
        cat "$TMP" > "$LIMINE_DROPIN"
        rm -f "$TMP"
        if ! grep -qE '^[[:space:]]*KERNEL_CMDLINE\[default\]\+=' "$LIMINE_DROPIN"; then
            rm -f -- "$LIMINE_DROPIN"
            echo "[OK] Removed empty Limine entry-tool drop-in"
        else
            echo "[OK] Removed persistent Limine entry-tool kernel parameter"
        fi
    else
        echo "[OK] Limine entry-tool drop-in has no matching kernel parameter (skipped)"
    fi
else
    echo "[OK] Limine entry-tool drop-in not present (skipped)"
fi

# 4. Patch /boot/limine.conf — remove this connector mapping and consolidate
#    any remaining mappings into a single drm.edid_firmware parameter.
if [ -f "$LIMINE" ]; then
    if grep -qF -- "drm.edid_firmware=" "$LIMINE" 2>/dev/null; then
        backup_file "$LIMINE"
        TMP="$(mktemp /tmp/drmcru-limine.XXXXXX)"
        awk -v connector="$CONNECTOR" '
            function add_mapping(mapping) {{
                if (mapping == "" || index(mapping, connector ":") == 1)
                    return
                if (!seen[mapping]++)
                    mappings[++mapping_count] = mapping
            }}
            /^[[:space:]]*cmdline:/ {{
                delete seen
                delete mappings
                mapping_count = 0
                prefix = substr($0, 1, index($0, ":"))
                other = ""
                for (i = 2; i <= NF; i++) {{
                    if (index($i, "drm.edid_firmware=") == 1) {{
                        value = substr($i, length("drm.edid_firmware=") + 1)
                        count = split(value, values, ",")
                        for (j = 1; j <= count; j++)
                            add_mapping(values[j])
                    }} else {{
                        other = other (other == "" ? "" : " ") $i
                    }}
                }}
                combined = ""
                for (i = 1; i <= mapping_count; i++)
                    combined = combined (combined == "" ? "" : ",") mappings[i]
                print prefix (other == "" ? "" : " " other) \
                    (combined == "" ? "" : " drm.edid_firmware=" combined)
                next
            }}
            {{ print }}
        ' "$LIMINE" > "$TMP"
        cat "$TMP" > "$LIMINE"
        rm -f "$TMP"
        echo "[OK] Removed connector mapping from Limine cmdline entries"
    else
        echo "[OK] Kernel parameter not present in Limine config (skipped)"
    fi
else
    echo "[ERR] /boot/limine.conf not found — automatic uninstall supports Limine only" >&2
    exit 1
fi

# 5. Rebuild initramfs/UKIs.
if command -v limine-mkinitcpio >/dev/null 2>&1; then
    echo "[..] Rebuilding Limine initramfs/UKI entries (limine-mkinitcpio)..."
    limine-mkinitcpio
    echo "[OK] Limine initramfs/UKI entries rebuilt"
elif compgen -G "/etc/mkinitcpio.d/*.preset" >/dev/null; then
    echo "[..] Rebuilding initramfs (mkinitcpio -P)..."
    mkinitcpio -P
    echo "[OK] Initramfs rebuilt"
else
    echo "[ERR] No initramfs rebuild backend found" >&2
    exit 1
fi

trap - ERR

echo ""
echo "=== Done! Reboot to return to the monitor's normal EDID. ==="
"##
    )
}

/// Run the install. Returns a report on success, or an error.
pub fn install(plan: &InstallPlan) -> Result<InstallReport, InstallError> {
    validate_firmware_file_name(&plan.edid_file_name)?;

    // Validate source exists
    if !plan.edid_source.exists() {
        return Err(InstallError::MissingEdid(plan.edid_source.clone()));
    }

    let support = inspect_system_support();
    if !support.is_supported() {
        return Err(InstallError::UnsupportedSystem(support.report_text()));
    }

    run_privileged_script(&build_install_script(plan))
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
        let code = result.status.code().unwrap_or(-1);
        // pkexec returns 126 when the user dismisses the auth dialog
        if code == 126 {
            Err(InstallError::Cancelled)
        } else {
            Err(InstallError::ScriptFailed { code, output })
        }
    }
}

fn validate_firmware_file_name(edid_file_name: &str) -> Result<(), InstallError> {
    if edid_file_name.is_empty() || edid_file_name.contains('/') || edid_file_name.contains('\\') {
        return Err(InstallError::UnsafeFirmwareName(edid_file_name.to_string()));
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
                file.write_all(script.as_bytes())
                    .map_err(InstallError::ScriptWrite)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> InstallPlan {
        InstallPlan {
            connector: "DP-1".to_string(),
            edid_source: PathBuf::from("/tmp/drmcru_custom_DP-1.bin"),
            edid_file_name: "drmcru_custom_DP-1.bin".to_string(),
            kernel_parameter: "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin".to_string(),
        }
    }

    fn sample_uninstall_plan() -> UninstallPlan {
        UninstallPlan {
            connector: "DP-1".to_string(),
            edid_file_name: "drmcru_custom_DP-1.bin".to_string(),
            kernel_parameter: "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin".to_string(),
        }
    }

    fn unique_test_dir() -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("drmcru-install-test-{}-{now}", std::process::id()))
    }

    #[test]
    fn system_support_accepts_limine_mkinitcpio_stack() {
        let dir = unique_test_dir();
        let mkinit_dir = dir.join("mkinitcpio.d");
        fs::create_dir_all(&mkinit_dir).unwrap();
        let mkinit = dir.join("mkinitcpio.conf");
        let limine = dir.join("limine.conf");
        fs::write(&mkinit, "FILES=()\n").unwrap();
        fs::write(mkinit_dir.join("linux.preset"), "preset\n").unwrap();
        fs::write(&limine, "cmdline: quiet\n").unwrap();

        let report =
            inspect_system_support_with_paths(&mkinit, &mkinit_dir, &limine, true, true, false);

        assert!(report.is_supported());
        assert!(report.blocking_issues().is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_support_accepts_limine_mkinitcpio_without_presets() {
        let dir = unique_test_dir();
        let mkinit_dir = dir.join("mkinitcpio.d");
        fs::create_dir_all(&mkinit_dir).unwrap();
        let mkinit = dir.join("mkinitcpio.conf");
        let limine = dir.join("limine.conf");
        fs::write(&mkinit, "FILES=()\n").unwrap();
        fs::write(&limine, "cmdline: quiet\n").unwrap();

        let report =
            inspect_system_support_with_paths(&mkinit, &mkinit_dir, &limine, true, true, true);

        assert!(report.is_supported());
        assert!(report.blocking_issues().is_empty());
        assert_eq!(report.rebuild_backend_label(), "limine-mkinitcpio");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_support_blocks_missing_limine_cmdline() {
        let dir = unique_test_dir();
        let mkinit_dir = dir.join("mkinitcpio.d");
        fs::create_dir_all(&mkinit_dir).unwrap();
        let mkinit = dir.join("mkinitcpio.conf");
        let limine = dir.join("limine.conf");
        fs::write(&mkinit, "FILES=()\n").unwrap();
        fs::write(mkinit_dir.join("linux.preset"), "preset\n").unwrap();
        fs::write(&limine, "timeout: 3\n").unwrap();

        let report =
            inspect_system_support_with_paths(&mkinit, &mkinit_dir, &limine, true, true, false);

        assert!(!report.is_supported());
        assert!(
            report
                .blocking_issues()
                .iter()
                .any(|issue| issue.contains("no cmdline"))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn script_contains_all_steps() {
        let script = build_install_script(&sample_plan());
        assert!(script.contains("install -D -m 0644"));
        assert!(script.contains("/lib/firmware/edid/drmcru_custom_DP-1.bin"));
        assert!(script.contains("mkinitcpio.conf"));
        assert!(script.contains("drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin"));
        assert!(script.contains("limine.conf"));
        assert!(script.contains("limine-entry-tool.d"));
        assert!(script.contains("drmcru-edid.conf"));
        assert!(script.contains("limine-mkinitcpio"));
        assert!(script.contains("mkinitcpio -P"));
    }

    #[test]
    fn script_is_idempotent() {
        let script = build_install_script(&sample_plan());
        // The script checks mkinitcpio and de-duplicates connector mappings.
        assert!(script.contains("grep -qF"));
        assert!(script.contains("if (!seen[mapping]++)"));
        assert!(script.contains("index(mapping, connector \":\") == 1"));
    }

    #[test]
    fn script_creates_backups_before_config_edits() {
        let script = build_install_script(&sample_plan());
        assert!(script.contains("BACKUP_SUFFIX"));
        assert!(script.contains("backup_file \"$MKINIT\""));
        assert!(script.contains("backup_file \"$LIMINE\""));
        assert!(script.contains("backup_file \"$LIMINE_DROPIN\""));
    }

    #[test]
    fn script_shell_quotes_interpolated_values() {
        let plan = InstallPlan {
            connector: "DP-1".to_string(),
            edid_source: PathBuf::from("/tmp/hypr cru's.bin"),
            edid_file_name: "hypr cru's.bin".to_string(),
            kernel_parameter: "drm.edid_firmware=DP-1:edid/hypr cru's.bin".to_string(),
        };
        let script = build_install_script(&plan);

        assert!(script.contains("EDID_SOURCE='/tmp/hypr cru'\\''s.bin'"));
        assert!(script.contains("FIRMWARE_TARGET='/lib/firmware/edid/hypr cru'\\''s.bin'"));
        assert!(script.contains("KERNEL_PARAM='drm.edid_firmware=DP-1:edid/hypr cru'\\''s.bin'"));
    }

    #[test]
    fn validation_rejects_firmware_names_with_path_separators() {
        assert!(matches!(
            validate_firmware_file_name("../escape.bin"),
            Err(InstallError::UnsafeFirmwareName(_))
        ));
    }

    #[test]
    fn uninstall_script_removes_edid_references() {
        let script = build_uninstall_script(&sample_uninstall_plan());
        assert!(script.contains("rm -f -- \"$FIRMWARE_TARGET\""));
        assert!(script.contains("Removed EDID from mkinitcpio FILES"));
        assert!(script.contains("Removed persistent Limine entry-tool kernel parameter"));
        assert!(script.contains("Removed connector mapping from Limine cmdline entries"));
        assert!(script.contains("limine-mkinitcpio"));
        assert!(script.contains("mkinitcpio -P"));
    }

    #[test]
    fn install_script_preflights_supported_stack_before_mutating() {
        let script = build_install_script(&sample_plan());
        let mkinit_check = script
            .find("[ERR] /etc/mkinitcpio.conf not found")
            .expect("mkinitcpio config preflight");
        let limine_check = script
            .find("[ERR] /boot/limine.conf not found")
            .expect("Limine config preflight");
        let cmdline_check = script
            .find("[ERR] /boot/limine.conf has no cmdline entries")
            .expect("Limine cmdline preflight");
        let rebuild_backend_check = script
            .find("[ERR] No initramfs rebuild backend found")
            .expect("initramfs backend preflight");
        let command_check = script
            .find("[ERR] mkinitcpio was not found")
            .expect("mkinitcpio command preflight");
        let first_mutation = script
            .find("install -D -m 0644")
            .expect("firmware install mutation");

        assert!(mkinit_check < first_mutation);
        assert!(limine_check < first_mutation);
        assert!(cmdline_check < first_mutation);
        assert!(rebuild_backend_check < first_mutation);
        assert!(command_check < first_mutation);
    }

    #[test]
    fn install_script_updates_missing_limine_cmdlines_instead_of_global_skip() {
        let script = build_install_script(&sample_plan());

        assert!(script.contains("Consolidated EDID mappings in all Limine cmdline entries"));
        assert!(script.contains("combined == \"\" ? \"\" : \",\""));
        assert!(!script.contains("Kernel parameter already in Limine config (skipped)"));
    }

    #[test]
    fn mapping_detection_handles_consolidated_kernel_parameter() {
        let dp1 = "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin";
        let cmdline = "quiet drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin,eDP-1:edid/drmcru_custom_eDP-1.bin splash";

        assert!(text_contains_kernel_mapping(cmdline, dp1));
        assert!(text_contains_kernel_mapping(
            cmdline,
            "drm.edid_firmware=eDP-1:edid/drmcru_custom_eDP-1.bin"
        ));
        assert!(!text_contains_kernel_mapping(
            cmdline,
            "drm.edid_firmware=DP-2:edid/drmcru_custom_DP-2.bin"
        ));
    }

    #[test]
    fn generated_privileged_scripts_are_bash_parseable() {
        assert_bash_n(&build_install_script(&sample_plan()));
        assert_bash_n(&build_uninstall_script(&sample_uninstall_plan()));
    }

    fn assert_bash_n(script: &str) {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let script_path = dir.join("script.sh");
        fs::write(&script_path, script).unwrap();

        let output = std::process::Command::new("bash")
            .arg("-n")
            .arg(&script_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_script_preflights_supported_stack_before_mutating() {
        let script = build_uninstall_script(&sample_uninstall_plan());
        let mkinit_check = script
            .find("[ERR] /etc/mkinitcpio.conf not found")
            .expect("mkinitcpio config preflight");
        let limine_check = script
            .find("[ERR] /boot/limine.conf not found")
            .expect("Limine config preflight");
        let cmdline_check = script
            .find("[ERR] /boot/limine.conf has no cmdline entries")
            .expect("Limine cmdline preflight");
        let rebuild_backend_check = script
            .find("[ERR] No initramfs rebuild backend found")
            .expect("initramfs backend preflight");
        let command_check = script
            .find("[ERR] mkinitcpio was not found")
            .expect("mkinitcpio command preflight");
        let first_mutation = script
            .find("rm -f -- \"$FIRMWARE_TARGET\"")
            .expect("firmware removal mutation");

        assert!(mkinit_check < first_mutation);
        assert!(limine_check < first_mutation);
        assert!(cmdline_check < first_mutation);
        assert!(rebuild_backend_check < first_mutation);
        assert!(command_check < first_mutation);
        assert!(!script.contains("[WARN]"));
    }

    #[test]
    fn uninstall_preview_has_all_targets() {
        let preview = UninstallPreview::from_plan(&sample_uninstall_plan());
        let lines = preview.summary_lines();
        assert_eq!(lines.len(), 7);
        assert!(lines[0].contains("DP-1"));
        assert!(lines[1].contains("Remove"));
        assert!(lines[2].contains("mkinitcpio"));
        assert!(lines[3].contains("limine"));
        assert!(lines[4].contains("limine-entry-tool"));
        assert!(lines[5].contains("drm.edid_firmware"));
        assert!(lines[6].contains("limine-mkinitcpio"));
    }

    #[test]
    fn installed_override_status_detects_active_install() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let firmware = dir.join("drmcru_custom_DP-1.bin");
        let mkinit = dir.join("mkinitcpio.conf");
        let limine = dir.join("limine.conf");
        let limine_dropin = dir.join("drmcru-edid.conf");
        let cmdline = dir.join("cmdline");
        let param = "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin";
        fs::write(&firmware, b"edid").unwrap();
        fs::write(&mkinit, format!("FILES=({})\n", firmware.display())).unwrap();
        fs::write(&limine, format!("cmdline: quiet {param}\n")).unwrap();
        fs::write(
            &limine_dropin,
            format!("KERNEL_CMDLINE[default]+=\" {param}\"\n"),
        )
        .unwrap();
        fs::write(&cmdline, format!("root=/dev/sda1 {param}\n")).unwrap();

        let status = inspect_override_with_paths(
            "DP-1",
            "drmcru_custom_DP-1.bin",
            param,
            OverrideInspectionPaths {
                firmware_target: &firmware,
                mkinitcpio_conf: &mkinit,
                bootloader_conf: &limine,
                limine_entry_tool_dropin: &limine_dropin,
                active_cmdline: &cmdline,
            },
        );

        assert!(status.firmware_present);
        assert!(status.mkinitcpio_references_firmware);
        assert!(status.bootloader_references_kernel_parameter);
        assert!(status.limine_entry_tool_references_kernel_parameter);
        assert_eq!(status.bootloader_cmdline_entries, 1);
        assert_eq!(status.bootloader_cmdline_entries_with_kernel_parameter, 1);
        assert!(status.active_kernel_references_kernel_parameter);
        assert_eq!(status.short_label(), "active");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_override_status_detects_missing_override() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let firmware = dir.join("drmcru_custom_DP-1.bin");
        let mkinit = dir.join("mkinitcpio.conf");
        let limine = dir.join("limine.conf");
        let limine_dropin = dir.join("drmcru-edid.conf");
        let cmdline = dir.join("cmdline");
        let param = "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin";
        fs::write(&mkinit, "FILES=()\n").unwrap();
        fs::write(&limine, "cmdline: quiet\n").unwrap();
        fs::write(&cmdline, "root=/dev/sda1\n").unwrap();

        let status = inspect_override_with_paths(
            "DP-1",
            "drmcru_custom_DP-1.bin",
            param,
            OverrideInspectionPaths {
                firmware_target: &firmware,
                mkinitcpio_conf: &mkinit,
                bootloader_conf: &limine,
                limine_entry_tool_dropin: &limine_dropin,
                active_cmdline: &cmdline,
            },
        );

        assert!(!status.has_any_override());
        assert_eq!(status.short_label(), "not installed");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_override_status_detects_partial_limine_cmdline_install() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let firmware = dir.join("drmcru_custom_DP-1.bin");
        let mkinit = dir.join("mkinitcpio.conf");
        let limine = dir.join("limine.conf");
        let limine_dropin = dir.join("drmcru-edid.conf");
        let cmdline = dir.join("cmdline");
        let param = "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin";
        fs::write(&firmware, b"edid").unwrap();
        fs::write(&mkinit, format!("FILES=({})\n", firmware.display())).unwrap();
        fs::write(&limine, format!("cmdline: quiet\ncmdline: quiet {param}\n")).unwrap();
        fs::write(&cmdline, "root=/dev/sda1\n").unwrap();

        let status = inspect_override_with_paths(
            "DP-1",
            "drmcru_custom_DP-1.bin",
            param,
            OverrideInspectionPaths {
                firmware_target: &firmware,
                mkinitcpio_conf: &mkinit,
                bootloader_conf: &limine,
                limine_entry_tool_dropin: &limine_dropin,
                active_cmdline: &cmdline,
            },
        );

        assert!(status.bootloader_references_kernel_parameter);
        assert_eq!(status.bootloader_cmdline_entries, 2);
        assert_eq!(status.bootloader_cmdline_entries_with_kernel_parameter, 1);
        assert!(!status.is_configured_for_next_boot());
        assert_eq!(status.short_label(), "partial install");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn firmware_comparison_detects_matching_live_edid() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let firmware = dir.join("drmcru_custom_DP-1.bin");
        fs::write(&firmware, b"edid-bytes").unwrap();

        let comparison = compare_firmware_to_live_edid(&firmware, Some(b"edid-bytes"));

        assert_eq!(comparison, FirmwareEdidComparison::Matches { bytes: 10 });
        assert_eq!(comparison.short_label(), "matches");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn firmware_comparison_detects_different_live_edid() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let firmware = dir.join("drmcru_custom_DP-1.bin");
        fs::write(&firmware, b"firmware").unwrap();

        let comparison = compare_firmware_to_live_edid(&firmware, Some(b"live"));

        assert_eq!(
            comparison,
            FirmwareEdidComparison::Differs {
                firmware_bytes: 8,
                live_bytes: 4,
            }
        );
        assert!(comparison.detail().contains("differs"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_summary_has_all_targets() {
        let preview = InstallPreview::from_plan(&sample_plan());
        let lines = preview.summary_lines();
        assert!(lines.len() == 7);
        assert!(lines[0].contains("DP-1"));
        assert!(lines[1].contains("firmware"));
        assert!(lines[2].contains("mkinitcpio"));
        assert!(lines[3].contains("limine"));
        assert!(lines[4].contains("limine-entry-tool"));
        assert!(lines[5].contains("drm.edid_firmware"));
        assert!(lines[6].contains("limine-mkinitcpio"));
    }
}
