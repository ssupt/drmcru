# Shared by the generated install and uninstall scripts. The caller supplies
# the literal-array editor and backup/rollback functions.
check_mkinitcpio_dropins() {
    if { [ -e "$MKINIT_DROPIN" ] || [ -L "$MKINIT_DROPIN" ]; } && [ ! -f "$MKINIT_DROPIN" ]; then
        echo "[ERR] $MKINIT_DROPIN must be a regular configuration file" >&2
        return 1
    fi
    if ! command -v limine-mkinitcpio >/dev/null 2>&1; then
        local preset
        for preset in /etc/mkinitcpio.d/*.preset; do
            [ -f "$preset" ] || continue
            awk "$PRESET_CONFIG_AWK" "$preset" || return 1
        done
    fi
}

remove_legacy_mkinitcpio_entry() {
    # New installs leave the user's shell config alone. Only previously
    # installed literal entries need the legacy parser.
    if grep -qF -- "$FIRMWARE_TARGET" "$MKINIT"; then
        local tmp
        tmp="$(mktemp /tmp/drmcru-mkinitcpio.XXXXXX)"
        awk -v path="$FIRMWARE_TARGET" -v operation=remove "$FILES_AWK" "$MKINIT" > "$tmp"
        if ! cmp -s -- "$tmp" "$MKINIT"; then
            backup_file "$MKINIT"
            cat "$tmp" > "$MKINIT"
            echo "[OK] Removed legacy EDID entry from /etc/mkinitcpio.conf"
        fi
        rm -f -- "$tmp"
    fi
}

update_mkinitcpio_dropin() {
    local operation="$1" tmp
    if [ "$operation" = add ]; then
        mkdir -p -- "$MKINIT_DROPIN_DIR"
    elif [ ! -f "$MKINIT_DROPIN" ]; then
        return
    fi
    if [ -f "$MKINIT_DROPIN" ]; then
        backup_file "$MKINIT_DROPIN"
    fi
    tmp="$(mktemp /tmp/drmcru-mkinitcpio.XXXXXX)"
    if [ -f "$MKINIT_DROPIN" ]; then
        awk -v path="$FIRMWARE_TARGET" -v operation="$operation" -v append=1 "$FILES_AWK" "$MKINIT_DROPIN" > "$tmp"
    else
        printf '# EDID overrides managed by drmcru.\n' > "$tmp"
        awk -v path="$FIRMWARE_TARGET" -v operation=add -v append=1 "$FILES_AWK" /dev/null >> "$tmp"
    fi
    # Delete our file only when it contains comments and empty append arrays.
    # Keep other connectors and any additional settings intact.
    if [ "$operation" = remove ] && awk '
        /^[[:space:]]*(#|$)/ { next }
        /^[[:space:]]*FILES\+=\([[:space:]]*\)[[:space:]]*$/ { next }
        { nonempty = 1 }
        END { exit nonempty ? 1 : 0 }
    ' "$tmp"; then
        rm -f -- "$MKINIT_DROPIN"
    else
        install -m 0644 -- "$tmp" "$MKINIT_DROPIN"
    fi
    rm -f -- "$tmp"
}
