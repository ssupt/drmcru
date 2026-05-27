# drmcru

`drmcru` is a Linux DRM/KMS custom resolution utility.

It edits monitor EDID data, exports patched EDID binaries, and can install those
overrides on supported systems so the kernel exposes custom modes after reboot.
Hyprland integration is included for live mode discovery, switching, verification,
and generated `monitor=...` rules.

## Status

This release has been tested on:

- Hyprland
- Limine
- `limine-mkinitcpio`
- external DisplayPort monitor

Exported EDIDs can be installed manually on other setups. Automatic
Install/Update/Uninstall is currently limited to Limine systems that rebuild with
`limine-mkinitcpio` or mkinitcpio presets.

Use known-good timings and keep a fallback display path available.

## Build

```sh
cargo build --release
./target/release/drmcru doctor
./target/release/drmcru
```

Useful commands:

```sh
cargo run
cargo run -- doctor
cargo run -- --help
cargo run -- --version
```

## Basic Workflow

1. Select a monitor.
2. Add, edit, copy, or paste a Detailed Resolution.
3. Export the patched EDID, or Install/Update it on a supported system.
4. Reboot.
5. Run `drmcru doctor`.
6. If Hyprland exposes the mode, use Switch or persist the generated
   `monitor=...` rule in your Hyprland config.

`Switch` only selects modes already exposed by DRM/Hyprland. It does not make a
new EDID mode appear.

## What It Can Edit

- Established timing bits
- Base-block detailed timing descriptors
- Base-block standard timings
- CTA-861 extension detailed timing descriptors

Detailed timings are the right place for custom modes such as `1280x1080@240`.
EDID standard timings have fixed aspect-ratio limits and are not suitable for
arbitrary shapes.

## Apply Model

Wayland compositors do not work like X11 modeline injection. For reliable custom
modes, the kernel must see the mode in the connector EDID.

`drmcru` writes EDID overrides for use with:

```text
drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin
```

On the supported Limine/mkinitcpio path, Install/Update modifies:

- `/lib/firmware/edid/<name>.bin`
- `/etc/mkinitcpio.conf`
- `/boot/limine.conf`
- `/etc/limine-entry-tool.d/drmcru-edid.conf` when `limine-mkinitcpio` is used

It writes timestamped `.drmcru.*.bak` backups before editing config files.

## Hyprland

When `hyprctl` is available, `drmcru` can:

- merge DRM connector data with `hyprctl monitors -j`
- list Hyprland's exposed modes
- switch to an already exposed mode
- verify the active mode
- inspect simple Hyprland `monitor=` rules and sourced config files

`drmcru` does not auto-edit Hyprland config.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
