use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::symlink;

const MACHINE: &str = "00000000000000000000000000000000";
const BASE_ARGS: &str = "root=UUID=test cryptdevice=UUID=crypt:root quiet";
fn sample_plan() -> InstallPlan {
    InstallPlan {
        connector: "DP-1".into(),
        edid_source: "/tmp/source.bin".into(),
        edid_file_name: "drmcru_custom_DP-1.bin".into(),
        kernel_parameter: "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin".into(),
    }
}
fn sample_uninstall_plan() -> UninstallPlan {
    let plan = sample_plan();
    UninstallPlan {
        connector: plan.connector,
        edid_file_name: plan.edid_file_name,
        kernel_parameter: plan.kernel_parameter,
    }
}
fn unique_test_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "drmcru-transaction-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
struct ScriptFixture {
    dir: PathBuf,
}
impl ScriptFixture {
    fn new() -> Self {
        let fixture = Self {
            dir: unique_test_dir(),
        };
        for path in [
            "bin",
            "boot",
            "firmware-root/edid",
            "entry-tool",
            "tmp",
            "locks",
            "modules/test-6.0",
            "drm/card0-DP-1",
            "hooks/pre.d",
            "hooks/post.d",
        ] {
            fs::create_dir_all(fixture.dir.join(path)).unwrap();
        }
        fs::write(fixture.dir.join("source.bin"), b"new-edid").unwrap();
        fs::write(fixture.dir.join("mkinitcpio.conf"), "FILES=()\n").unwrap();
        fs::write(fixture.dir.join("machine-id"), MACHINE).unwrap();
        fs::write(fixture.dir.join("kernel-cmdline"), BASE_ARGS).unwrap();
        fs::write(fixture.dir.join("modules/test-6.0/modules.builtin"), "").unwrap();
        fs::write(fixture.dir.join("modules/test-6.0/pkgbase"), "linux-test").unwrap();
        fs::write(fixture.dir.join("modules/test-6.0/vmlinuz"), b"new-kernel").unwrap();
        fs::write(fixture.dir.join("drm/card0-DP-1/status"), "connected").unwrap();
        fs::write(fixture.dir.join("entry-base.conf"), "").unwrap();
        fixture.kernel_entry("linux-test", BASE_ARGS);
        symlink(
            fixture.dir.join("boot/limine.conf"),
            fixture.dir.join("limine.conf"),
        )
        .unwrap();
        fs::write(
            fixture.dir.join("common-functions"),
            r#"
load_config() {
    ESP_PATH="$TEST_ROOT/boot"
    ENABLE_UKI="$TEST_UKI"
    MKINITCPIO_FALLBACK="$TEST_FALLBACK"
}
check_boot_partition() { return 0; }
is_uefi() { [ "$TEST_UKI" = yes ]; }
is_sb_installed() { [ "$TEST_NO_EMBED" = yes ]; }
is_snapper_installed() { [ "$TEST_NO_EMBED" = yes ]; }
"#,
        )
        .unwrap();
        fixture.command("id", "printf '0\\n'");
        fixture.command(
            "limine-mkinitcpio",
            "echo wrapper-must-not-run >&2; exit 99",
        );
        fixture.command("pacman", "cat \"$(dirname \"$2\")/pkgbase\"");
        fixture.command("findmnt", "printf '%s\\n' \"$TEST_ROOT/boot\"");
        fixture.command("lsinitcpio", "tar -xf \"$2\"");
        fixture.command(
            "objcopy",
            r#"
previous=""; source=""
for argument in "$@"; do source="$previous"; previous="$argument"; done
target="$previous"
# Drain the listing: grep -q can close early and make tar fail with SIGPIPE.
if tar -tf "$source" | grep -x './.cmdline' >/dev/null; then
    tar -xOf "$source" ./.cmdline > "$target"
else : > "$target"; fi
"#,
        );
        fixture.write_executable(
            "limine-entry-tool",
            include_str!("../tests/fixtures/limine-entry-tool.py"),
        );
        fixture.write_executable(
            "mkinitcpio",
            include_str!("../tests/fixtures/mkinitcpio.sh"),
        );
        fixture
    }
    fn kernel_entry(&self, name: &str, arguments: &str) {
        let folder = self.dir.join(format!("boot/{MACHINE}/{name}"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("vmlinuz"), b"old-kernel").unwrap();
        fs::write(folder.join("initramfs"), b"old-image").unwrap();
        let path = self.dir.join("boot/limine.conf");
        let mut contents = fs::read_to_string(&path).unwrap_or_default();
        contents.push_str(&format!("/{name}\n  protocol: linux\n  path: boot():/{MACHINE}/{name}/vmlinuz\n  module_path: boot():/{MACHINE}/{name}/initramfs\n  cmdline: {arguments}\n"));
        fs::write(path, contents).unwrap();
    }
    fn write_executable(&self, name: &str, body: &str) {
        let path = self.dir.join("bin").join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn command(&self, name: &str, body: &str) {
        self.write_executable(name, &format!("#!/bin/bash\nset -euo pipefail\n{body}\n"));
    }
    fn run(&self, uninstall: bool) -> std::process::Output {
        self.run_options(uninstall, "DP-1", &[])
    }
    fn run_options(
        &self,
        uninstall: bool,
        connector: &str,
        environment: &[(&str, &str)],
    ) -> std::process::Output {
        let mut plan = sample_plan();
        plan.connector = connector.into();
        plan.edid_file_name = format!("drmcru_custom_{connector}.bin");
        plan.edid_source = self.dir.join("source.bin");
        let mut script = if uninstall {
            build_uninstall_script(&UninstallPlan {
                connector: plan.connector,
                edid_file_name: plan.edid_file_name,
                kernel_parameter: plan.kernel_parameter,
            })
        } else {
            build_install_script(&plan)
        };
        for (variable, original, relative) in [
            ("MKINITCPIO_BIN", "/usr/bin/mkinitcpio", "bin/mkinitcpio"),
            ("LSINITCPIO_BIN", "/usr/bin/lsinitcpio", "bin/lsinitcpio"),
            (
                "ENTRY_TOOL_BIN",
                "/usr/bin/limine-entry-tool",
                "bin/limine-entry-tool",
            ),
            ("OBJCOPY_BIN", "/usr/bin/objcopy", "bin/objcopy"),
            ("FIRMWARE_ROOT", "/lib/firmware", "firmware-root"),
            ("BOOT_ROOT", "/boot", "boot"),
            ("MKINIT", "/etc/mkinitcpio.conf", "mkinitcpio.conf"),
            (
                "LIMINE_DROPIN_DIR",
                "/etc/limine-entry-tool.d",
                "entry-tool",
            ),
            (
                "LIMINE_CONFIG",
                "/etc/limine-entry-tool.conf",
                "entry-base.conf",
            ),
            (
                "LIMINE_DEFAULT_CONFIG",
                "/etc/default/limine",
                "default.conf",
            ),
            (
                "LIMINE_VENDOR_DIR",
                "/usr/share/limine-entry-tool.d",
                "vendor",
            ),
            (
                "LIMINE_FUNCTIONS",
                "/usr/lib/limine/limine-common-functions",
                "common-functions",
            ),
            ("BOOT_HOOKS", "/etc/boot/hooks", "hooks"),
            (
                "BOOT_LOCK",
                "/run/lock/boot-partition.lock",
                "locks/boot.lock",
            ),
            (
                "PACKAGE_LOCK",
                "/var/lib/pacman/db.lck",
                "locks/package.lock",
            ),
            ("TRANSACTION_ROOT", "/var/tmp", "tmp"),
            ("MODULES_ROOT", "/usr/lib/modules", "modules"),
            ("MACHINE_ID_FILE", "/etc/machine-id", "machine-id"),
            ("DRM_ROOT", "/sys/class/drm", "drm"),
        ] {
            script = script.replace(
                &format!("{variable}=\"{original}\""),
                &format!(
                    "{variable}={}",
                    shell_quote(&self.dir.join(relative).display().to_string())
                ),
            );
        }
        Command::new("bash")
            .args(["-c", &script])
            .env("TEST_ROOT", &self.dir)
            .env("TEST_UKI", "no")
            .env("TEST_FALLBACK", "no")
            .env("TEST_NO_EMBED", "no")
            .envs(environment.iter().copied())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.dir.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .output()
            .unwrap()
    }
    fn firmware(&self) -> PathBuf {
        self.dir.join("firmware-root/edid/drmcru_custom_DP-1.bin")
    }
    fn mkinitcpio_dropin(&self) -> PathBuf {
        self.dir.join("mkinitcpio.conf.d/20-drmcru.conf")
    }
    fn boot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, base: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(&path, base, files);
                } else {
                    files.insert(
                        path.strip_prefix(base).unwrap().to_path_buf(),
                        fs::read(&path).unwrap(),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        collect(&self.dir.join("boot"), &self.dir.join("boot"), &mut files);
        files
    }
}
impl Drop for ScriptFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn staged_install_preserves_fallback_arguments_and_keeps_recovery() {
    let fixture = ScriptFixture::new();
    success(&fixture.run(false));
    let menu = fs::read_to_string(fixture.dir.join("limine.conf")).unwrap();
    assert!(menu.contains(BASE_ARGS));
    assert!(menu.contains("drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin"));
    assert!(menu.contains("# BEGIN drmcru recovery"));
    assert_eq!(
        fs::read(fixture.dir.join("boot/drmcru-recovery/0-vmlinuz")).unwrap(),
        b"old-kernel"
    );
    assert_eq!(fs::read(fixture.firmware()).unwrap(), b"new-edid");
    assert!(!fixture.dir.join("locks/package.lock").exists());
}
#[test]
fn zero_exit_without_a_built_image_is_rejected_and_rolled_back() {
    let fixture = ScriptFixture::new();
    let before = fixture.boot();
    fs::write(fixture.dir.join("skip-build"), "").unwrap();
    let output = fixture.run(false);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Image was not produced"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fixture.boot(), before);
    assert!(!fixture.firmware().exists());
}
#[test]
fn outdated_firmware_in_an_image_is_rejected() {
    let fixture = ScriptFixture::new();
    let before = fixture.boot();
    fs::write(fixture.dir.join("wrong-firmware"), "").unwrap();
    assert!(!fixture.run(false).status.success());
    assert_eq!(fixture.boot(), before);
    assert!(!fixture.firmware().exists());
}
#[test]
fn failure_after_install_restores_boot_images_metadata_and_firmware() {
    for uninstall in [false, true] {
        let fixture = ScriptFixture::new();
        if uninstall {
            success(&fixture.run(false));
        }
        let before = fixture.boot();
        let firmware = fs::read(fixture.firmware()).ok();
        let config = fs::read(fixture.mkinitcpio_dropin()).ok();
        fs::write(fixture.dir.join("fail-after-install"), "").unwrap();
        let output = fixture.run(uninstall);
        assert!(!output.status.success());
        assert_eq!(fixture.boot(), before);
        assert_eq!(fs::read(fixture.firmware()).ok(), firmware);
        assert_eq!(fs::read(fixture.mkinitcpio_dropin()).ok(), config);
        assert!(!fixture.dir.join("locks/package.lock").exists());
    }
}
#[test]
fn a_post_hook_failure_restores_even_unrelated_boot_files() {
    let fixture = ScriptFixture::new();
    let before = fixture.boot();
    let hook = fixture.dir.join("hooks/post.d/fail");
    fs::write(
        &hook,
        "#!/bin/bash\nprintf changed > \"$TEST_ROOT/boot/extra-image\"\nexit 42\n",
    )
    .unwrap();
    fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!fixture.run(false).status.success());
    assert_eq!(fixture.boot(), before);
}
#[test]
fn external_and_per_kernel_overrides_are_consolidated_without_losing_arguments() {
    let fixture = ScriptFixture::new();
    fs::write(
        fixture.dir.join("firmware-root/edid/other.bin"),
        b"other-edid",
    )
    .unwrap();
    fs::write(fixture.dir.join("entry-base.conf"),
        "KERNEL_CMDLINE[default]=root=UUID=test quiet drm.edid_firmware=HDMI-A-1:edid/other.bin\nKERNEL_CMDLINE[linux-test]=root=UUID=special rootflags=\"subvol=/with space\" drm.edid_firmware=HDMI-A-1:edid/other.bin\n").unwrap();
    success(&fixture.run(false));
    let own = fs::read_to_string(fixture.dir.join("default.conf")).unwrap();
    let line = own
        .lines()
        .find(|line| line.starts_with("KERNEL_CMDLINE[linux-test]="))
        .unwrap();
    assert!(line.contains("root=UUID=special rootflags=\"subvol=/with space\""));
    assert!(line.contains("HDMI-A-1:edid/other.bin"));
    assert!(line.contains("DP-1:edid/drmcru_custom_DP-1.bin"));
    assert_eq!(line.matches("drm.edid_firmware=").count(), 1);
}
#[test]
fn legacy_edid_only_default_is_repaired_before_regeneration() {
    let fixture = ScriptFixture::new();
    fs::write(
        fixture.dir.join("entry-tool/drmcru-edid.conf"),
        "KERNEL_CMDLINE[default]+=\" drm.edid_firmware=DP-1:edid/old.bin\"\n",
    )
    .unwrap();
    success(&fixture.run(false));
    let own = fs::read_to_string(fixture.dir.join("default.conf")).unwrap();
    assert!(own.contains(BASE_ARGS));
    assert!(!own.contains("old.bin"));
}
#[test]
fn prefix_collision_is_rejected_before_any_file_changes() {
    let fixture = ScriptFixture::new();
    fs::create_dir_all(fixture.dir.join("drm/card0-DP-10")).unwrap();
    fs::write(fixture.dir.join("drm/card0-DP-10/status"), "disconnected").unwrap();
    let before = fixture.boot();
    let output = fixture.run(false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("also matches DP-10"));
    assert_eq!(fixture.boot(), before);
    assert!(!fixture.firmware().exists());
}
#[test]
fn uki_and_fallback_images_are_verified_before_commit() {
    let fixture = ScriptFixture::new();
    success(&fixture.run_options(
        false,
        "DP-1",
        &[("TEST_UKI", "yes"), ("TEST_FALLBACK", "yes")],
    ));
    assert!(
        fixture
            .dir
            .join(format!("boot/EFI/Linux/{MACHINE}_linux-test.efi"))
            .is_file()
    );
    assert!(
        fixture
            .dir
            .join(format!("boot/EFI/Linux/{MACHINE}_linux-test-fallback.efi"))
            .is_file()
    );
}
#[test]
fn repeated_updates_and_uninstall_keep_other_monitor_firmware() {
    let fixture = ScriptFixture::new();
    fs::create_dir_all(fixture.dir.join("drm/card0-HDMI-A-1")).unwrap();
    fs::write(fixture.dir.join("drm/card0-HDMI-A-1/status"), "connected").unwrap();
    success(&fixture.run(false));
    success(&fixture.run_options(false, "HDMI-A-1", &[]));
    success(&fixture.run(false));
    success(&fixture.run(true));
    let own = fs::read_to_string(fixture.dir.join("entry-tool/drmcru-edid.conf")).unwrap();
    assert!(!own.contains("DP-1:edid/drmcru_custom_DP-1.bin"));
    assert!(own.contains("HDMI-A-1:edid/drmcru_custom_HDMI-A-1.bin"));
    assert!(!fixture.firmware().exists());
    assert!(
        fixture
            .dir
            .join("firmware-root/edid/drmcru_custom_HDMI-A-1.bin")
            .exists()
    );
}

#[test]
fn highest_priority_appends_remain_once_and_user_lines_survive_uninstall() {
    let fixture = ScriptFixture::new();
    let user = "# user settings\nKERNEL_CMDLINE[default]+=root=UUID=encrypted\nKERNEL_CMDLINE[default]+=cryptdevice=UUID=disk:root\nKERNEL_CMDLINE[default]+=rootflags=\"subvol=/with space\"\n";
    fs::write(fixture.dir.join("default.conf"), user).unwrap();
    for _ in 0..2 {
        success(&fixture.run(false));
        let config = fs::read_to_string(fixture.dir.join("default.conf")).unwrap();
        assert!(config.starts_with(user));
        let line = config
            .lines()
            .find(|line| line.starts_with("KERNEL_CMDLINE[linux-test]="))
            .unwrap();
        assert_eq!(line.matches("root=UUID=encrypted").count(), 1);
        assert_eq!(line.matches("cryptdevice=UUID=disk:root").count(), 1);
        assert_eq!(line.matches("rootflags=").count(), 1);
    }
    success(&fixture.run(true));
    assert_eq!(
        fs::read_to_string(fixture.dir.join("default.conf")).unwrap(),
        user
    );
}

#[test]
fn update_picks_up_changes_to_the_underlying_kernel_command_line() {
    let fixture = ScriptFixture::new();
    success(&fixture.run(false));
    let updated = "root=UUID=new cryptdevice=UUID=newdisk:root rw";
    fs::write(fixture.dir.join("kernel-cmdline"), updated).unwrap();
    success(&fixture.run(false));
    let config = fs::read_to_string(fixture.dir.join("default.conf")).unwrap();
    assert!(config.contains(updated));
    assert!(!config.contains(BASE_ARGS));
}

#[test]
fn a_new_kernel_inherits_all_previously_managed_monitor_overrides() {
    let fixture = ScriptFixture::new();
    fs::create_dir_all(fixture.dir.join("drm/card0-HDMI-A-1")).unwrap();
    fs::write(fixture.dir.join("drm/card0-HDMI-A-1/status"), "connected").unwrap();
    success(&fixture.run(false));
    success(&fixture.run_options(false, "HDMI-A-1", &[]));
    fs::create_dir_all(fixture.dir.join("modules/test-6.1")).unwrap();
    for (file, contents) in [
        ("modules.builtin", ""),
        ("pkgbase", "linux-other"),
        ("vmlinuz", "second-kernel"),
    ] {
        fs::write(fixture.dir.join("modules/test-6.1").join(file), contents).unwrap();
    }
    success(&fixture.run(false));
    let config = fs::read_to_string(fixture.dir.join("default.conf")).unwrap();
    let line = config
        .lines()
        .find(|line| line.starts_with("KERNEL_CMDLINE[linux-other]="))
        .unwrap();
    assert!(line.contains("HDMI-A-1:edid/drmcru_custom_HDMI-A-1.bin"));
    assert!(line.contains("DP-1:edid/drmcru_custom_DP-1.bin"));
}

#[test]
fn consolidation_keeps_linux_first_connector_match_and_last_global_fallback() {
    let fixture = ScriptFixture::new();
    for file in ["first.bin", "fallback2.bin"] {
        fs::write(fixture.dir.join("firmware-root/edid").join(file), file).unwrap();
    }
    fs::write(fixture.dir.join("entry-base.conf"), "KERNEL_CMDLINE[default]=root=UUID=test drm.edid_firmware=HDMI-A-1:edid/first.bin,HDMI-A-1:edid/second.bin,edid/fallback1.bin,edid/fallback2.bin\n").unwrap();
    success(&fixture.run(false));
    let config = fs::read_to_string(fixture.dir.join("default.conf")).unwrap();
    assert!(config.contains("HDMI-A-1:edid/first.bin"));
    assert!(config.contains("edid/fallback2.bin"));
    assert!(!config.contains("second.bin"));
    assert!(!config.contains("fallback1.bin"));
}

#[test]
fn incomplete_rollback_keeps_recovery_files_and_does_not_recommend_reboot() {
    let fixture = ScriptFixture::new();
    fs::write(fixture.firmware(), b"old-edid").unwrap();
    fixture.command("cp", "for arg in \"$@\"; do if [ \"$arg\" = --remove-destination ]; then exit 42; fi; done\nexec /usr/bin/cp \"$@\"");
    fs::write(fixture.dir.join("fail-after-install"), "").unwrap();
    let output = fixture.run(false);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Recovery was incomplete. Do not reboot")
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Reboot to activate"));
    assert!(
        fs::read_dir(fixture.dir.join("tmp"))
            .unwrap()
            .any(|entry| entry.unwrap().path().join("boot/limine.conf").exists())
    );
}

#[test]
fn a_later_kernel_or_fallback_failure_restores_the_whole_boot_partition() {
    for marker in ["fail-second-build", "fail-second-install"] {
        for second_kernel in [false, true] {
            let fixture = ScriptFixture::new();
            if second_kernel {
                fs::create_dir_all(fixture.dir.join("modules/test-6.1")).unwrap();
                for (file, contents) in [
                    ("modules.builtin", ""),
                    ("pkgbase", "linux-other"),
                    ("vmlinuz", "second-kernel"),
                ] {
                    fs::write(fixture.dir.join("modules/test-6.1").join(file), contents).unwrap();
                }
                fixture.kernel_entry("linux-other", BASE_ARGS);
            }
            let before = fixture.boot();
            fs::write(fixture.dir.join(marker), "").unwrap();
            let env = if second_kernel {
                vec![]
            } else {
                vec![("TEST_FALLBACK", "yes")]
            };
            assert!(!fixture.run_options(false, "DP-1", &env).status.success());
            assert_eq!(fixture.boot(), before);
            assert!(!fixture.firmware().exists());
        }
    }
}

#[test]
fn successful_hooks_cannot_change_verified_kernels_arguments_or_recovery() {
    for change in [
        format!("printf changed > \"$TEST_ROOT/boot/{MACHINE}/linux-test/vmlinuz\""),
        "sed -i 's/root=UUID=test/root=UUID=wrong/g' \"$TEST_ROOT/boot/limine.conf\"".into(),
        "printf changed > \"$TEST_ROOT/boot/drmcru-recovery/0-vmlinuz\"".into(),
        "sed -i 's/drmcru recovery (before last change)/changed recovery/' \"$TEST_ROOT/boot/limine.conf\"".into(),
    ] {
        let fixture = ScriptFixture::new();
        let before = fixture.boot();
        let hook = fixture.dir.join("hooks/post.d/change");
        fs::write(&hook, format!("#!/bin/bash\n{change}\n")).unwrap();
        fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!fixture.run(false).status.success(), "{change}");
        assert_eq!(fixture.boot(), before);
    }
}

#[test]
fn corrupt_archives_and_wrong_uki_arguments_roll_back() {
    for marker in ["bad-archive", "wrong-cmdline"] {
        let fixture = ScriptFixture::new();
        let before = fixture.boot();
        fs::write(fixture.dir.join(marker), "").unwrap();
        assert!(
            !fixture
                .run_options(false, "DP-1", &[("TEST_UKI", "yes")])
                .status
                .success()
        );
        assert_eq!(fixture.boot(), before);
    }
}

#[test]
fn secure_boot_snapshot_ukis_can_omit_embedded_arguments() {
    let fixture = ScriptFixture::new();
    success(&fixture.run_options(
        false,
        "DP-1",
        &[("TEST_UKI", "yes"), ("TEST_NO_EMBED", "yes")],
    ));
}

#[test]
fn switching_boot_image_formats_keeps_recovery_and_replaces_stale_entries() {
    let fixture = ScriptFixture::new();
    success(&fixture.run_options(false, "DP-1", &[("TEST_UKI", "yes")]));
    assert!(
        !fixture
            .dir
            .join(format!("boot/{MACHINE}/linux-test"))
            .exists()
    );
    assert!(fixture.dir.join("boot/drmcru-recovery/0-vmlinuz").exists());
    success(&fixture.run(false));
    assert!(
        !fixture
            .dir
            .join(format!("boot/EFI/Linux/{MACHINE}_linux-test.efi"))
            .exists()
    );
    assert!(
        fixture
            .dir
            .join(format!("boot/{MACHINE}/linux-test/initramfs"))
            .exists()
    );
    let menu = fs::read_to_string(fixture.dir.join("limine.conf")).unwrap();
    assert!(menu.contains("drmcru recovery (before last change)"));
}

#[test]
fn failed_command_line_queries_stop_before_changes() {
    let fixture = ScriptFixture::new();
    let before = fixture.boot();
    fs::write(fixture.dir.join("fail-query"), "").unwrap();
    assert!(!fixture.run(false).status.success());
    assert_eq!(fixture.boot(), before);
    assert!(!fixture.firmware().exists());
    assert!(!fixture.dir.join("default.conf").exists());
}

#[test]
fn missing_or_duplicate_connectors_cannot_be_installed() {
    for duplicate in [false, true] {
        let fixture = ScriptFixture::new();
        if duplicate {
            fs::create_dir_all(fixture.dir.join("drm/card1-DP-1")).unwrap();
            fs::write(fixture.dir.join("drm/card1-DP-1/status"), "connected").unwrap();
        } else {
            fs::remove_file(fixture.dir.join("drm/card0-DP-1/status")).unwrap();
        }
        let before = fixture.boot();
        assert!(!fixture.run(false).status.success());
        assert_eq!(fixture.boot(), before);
        assert!(!fixture.firmware().exists());
    }
}

#[test]
fn uninstall_works_after_the_monitor_is_unplugged() {
    let fixture = ScriptFixture::new();
    success(&fixture.run(false));
    fs::remove_file(fixture.dir.join("drm/card0-DP-1/status")).unwrap();
    success(&fixture.run(true));
    assert!(!fixture.firmware().exists());
}

#[test]
fn legacy_firmware_migration_preserves_dynamic_config_and_other_monitors() {
    let fixture = ScriptFixture::new();
    let user = format!(
        "FILES=(\"{}\" /keep/other.bin)\nHOOKS=(base udev)\nCOMPRESSION=\"${{USER_COMPRESSION:-zstd}}\"\n",
        fixture.firmware().display()
    );
    fs::write(fixture.dir.join("mkinitcpio.conf"), user).unwrap();
    success(&fixture.run(false));
    let config = fs::read_to_string(fixture.dir.join("mkinitcpio.conf")).unwrap();
    assert!(config.contains("/keep/other.bin"));
    assert!(config.contains("HOOKS=(base udev)"));
    assert!(config.contains("${USER_COMPRESSION:-zstd}"));
    assert!(!config.contains(&fixture.firmware().display().to_string()));
}

#[test]
fn support_report_detects_explicit_preset_configs_without_executing_them() {
    let fixture = ScriptFixture::new();
    let presets = fixture.dir.join("presets");
    fs::create_dir_all(&presets).unwrap();
    let marker = fixture.dir.join("must-not-execute");
    for assignment in [
        "ALL_config='/etc/mkinitcpio.conf'".to_string(),
        "export default_config=\"/custom.conf\"".to_string(),
        format!(
            "ALL_config=$(touch {})",
            shell_quote(&marker.display().to_string())
        ),
    ] {
        fs::write(presets.join("linux.preset"), assignment).unwrap();
        let report = inspect_system_support_with_paths(
            &fixture.dir.join("mkinitcpio.conf"),
            &presets,
            &fixture.dir.join("limine.conf"),
            true,
            true,
            false,
        );
        assert!(!report.is_supported());
        assert!(
            report
                .report_text()
                .contains("explicit mkinitcpio config disables")
        );
        assert!(!marker.exists());
        // The Limine backend does not build through these presets.
        let report = inspect_system_support_with_paths(
            &fixture.dir.join("mkinitcpio.conf"),
            &presets,
            &fixture.dir.join("limine.conf"),
            true,
            true,
            true,
        );
        assert!(report.is_supported());
    }
}

#[test]
fn installed_override_status_finds_dropin_without_a_legacy_reference() {
    let fixture = ScriptFixture::new();
    let output = fixture.run(false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = inspect_override_with_paths(
        "DP-1",
        "drmcru_custom_DP-1.bin",
        &sample_plan().kernel_parameter,
        OverrideInspectionPaths {
            firmware_target: &fixture.firmware(),
            mkinitcpio_conf: &fixture.dir.join("mkinitcpio.conf"),
            mkinitcpio_dropin: &fixture.mkinitcpio_dropin(),
            bootloader_conf: &fixture.dir.join("limine.conf"),
            limine_entry_tool_dropin: &fixture.dir.join("entry-tool/drmcru-edid.conf"),
            active_cmdline: &fixture.dir.join("no-active-cmdline"),
        },
    );
    assert!(status.mkinitcpio_references_firmware);
    assert_eq!(status.short_label(), "installed, reboot pending");
    assert!(status.read_warnings.is_empty());
}

#[test]
fn system_support_rejects_preset_only_setups() {
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

    assert!(!report.is_supported());
    assert!(
        report
            .blocking_issues()
            .iter()
            .any(|issue| issue.contains("preset-only"))
    );

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

    let report = inspect_system_support_with_paths(&mkinit, &mkinit_dir, &limine, true, true, true);

    assert!(report.is_supported());
    assert!(report.blocking_issues().is_empty());
    assert_eq!(
        report.rebuild_backend_label(),
        "verified per-kernel mkinitcpio builds"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn system_support_defers_protected_limine_config_to_privileged_installer() {
    let limine = Path::new("/boot/limine.conf");

    let mut warnings = Vec::new();
    let result = inspect_limine_config_read(
        Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        limine,
        &mut warnings,
    );
    assert_eq!(result, (true, false, true));
    assert!(warnings.is_empty());

    let report = SystemSupportReport {
        pkexec_available: true,
        mkinitcpio_available: true,
        mkinitcpio_conf_present: true,
        mkinitcpio_presets_present: false,
        limine_mkinitcpio_available: true,
        limine_conf_present: result.0,
        limine_has_cmdline: result.1,
        limine_validation_deferred: result.2,
        read_warnings: warnings,
    };
    assert!(report.is_supported());
    assert!(
        report
            .summary_lines()
            .iter()
            .any(|line| line.contains("will verify after authentication"))
    );
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
fn validation_rejects_firmware_names_with_path_separators() {
    assert!(matches!(
        validate_firmware_file_name("../escape.bin"),
        Err(InstallError::UnsafeFirmwareName(_))
    ));
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
fn uninstall_preview_has_all_targets() {
    let preview = UninstallPreview::from_plan(&sample_uninstall_plan());
    let lines = preview.summary_lines();
    assert_eq!(lines.len(), 10);
    assert!(lines[0].contains("DP-1"));
    assert!(lines[1].contains("Remove"));
    assert!(lines[2].contains("mkinitcpio"));
    assert!(lines[3].contains("legacy"));
    assert!(lines[4].contains("limine"));
    assert!(lines[5].contains("limine-entry-tool"));
    assert!(lines[6].contains("/etc/default/limine"));
    assert!(lines[7].contains("drm.edid_firmware"));
    assert!(lines[8].contains("verify"));
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
            mkinitcpio_dropin: &dir.join("20-drmcru.conf"),
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
            mkinitcpio_dropin: &dir.join("20-drmcru.conf"),
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
            mkinitcpio_dropin: &dir.join("20-drmcru.conf"),
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
    assert_eq!(lines.len(), 10);
    assert!(lines[0].contains("DP-1"));
    assert!(lines[1].contains("firmware"));
    assert!(lines[2].contains("mkinitcpio"));
    assert!(lines[3].contains("legacy"));
    assert!(lines[4].contains("limine"));
    assert!(lines[5].contains("limine-entry-tool"));
    assert!(lines[6].contains("/etc/default/limine"));
    assert!(lines[7].contains("drm.edid_firmware"));
    assert!(lines[8].contains("verify"));
}

#[test]
fn pkexec_exit_codes_are_classified() {
    assert!(matches!(
        classify_failure(126, String::new()),
        InstallError::Cancelled
    ));
    assert!(matches!(
        classify_failure(127, String::new()),
        InstallError::NotAuthorized(_)
    ));
    assert!(matches!(
        classify_failure(1, String::new()),
        InstallError::ScriptFailed { code: 1, .. }
    ));
}

#[test]
fn script_failures_never_use_pkexec_reserved_exit_codes() {
    for code in [126, 127] {
        let fixture = ScriptFixture::new();
        let before = fixture.boot();
        fixture.command("mkinitcpio", &format!("exit {code}"));
        let output = fixture.run(false);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fixture.boot(), before);
    }
}
