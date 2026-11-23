# The generated prologue supplies paths, plans and the literal config editors.
set -Eeuo pipefail
shopt -s nullglob
shopt -s inherit_errexit
die() { echo "[ERR] $*" >&2; exit 1; }
declare -a BACKUP_ORIGINALS=() BACKUP_PATHS=() NEW_PATHS=() NEW_DIRS=()
declare -a KERNEL_NAMES=() KERNEL_VERSIONS=() KEYS=() IMAGES=() IMAGE_KEYS=() IMAGE_NAMES=() DESTINATIONS=()
declare -A KEY_SEEN=() ORIGINAL_CMDLINES=() EXPECTED_CMDLINES=()
TX=""; MUTATED=0; SNAPSHOT_READY=0; COMMITTED=0; PACKAGE_LOCK_OWNED=0
BACKUP_SUFFIX=".drmcru.$(date +%Y%m%d-%H%M%S-%N)-$$.bak"

backup_file() {
    local file="$1" previous
    [ ! -L "$file" ] || die "Configuration or firmware is a symlink: $file"
    for previous in "${BACKUP_ORIGINALS[@]}" "${NEW_PATHS[@]}"; do [ "$previous" != "$file" ] || return 0; done
    if [ -e "$file" ]; then
        [ -f "$file" ] && [ ! -L "$file" ] || die "Not a regular configuration file: $file"
        cp -a -- "$file" "$file$BACKUP_SUFFIX"
        BACKUP_ORIGINALS+=("$file"); BACKUP_PATHS+=("$file$BACKUP_SUFFIX")
    else
        NEW_PATHS+=("$file")
    fi
}

restore_boot() {
    local path relative original failed=0
    [ "$SNAPSHOT_READY" -eq 1 ] || return 0
    # Remove newly created objects and type changes without following links.
    while IFS= read -r -d '' path; do
        relative="${path#"$BOOT_ROOT/"}"; original="$TX/boot/$relative"
        if [ -d "$path" ] && [ ! -L "$path" ]; then
            if [ ! -d "$original" ]; then rmdir -- "$path" || failed=1; fi
        elif [ ! -f "$original" ] || [ -L "$path" ]; then
            rm -f -- "$path" || failed=1
        fi
    done < <(find "$BOOT_ROOT" -mindepth 1 -depth -print0)
    cp -a -- "$TX/boot/." "$BOOT_ROOT/" || failed=1
    diff -qr --no-dereference -- "$TX/boot" "$BOOT_ROOT" >/dev/null || failed=1
    return "$failed"
}

finish() {
    local status="$?" failed=0 i
    trap - EXIT HUP INT TERM
    set +e
    if [ "$COMMITTED" -eq 0 ] && [ "$MUTATED" -eq 1 ]; then
        for ((i=${#BACKUP_ORIGINALS[@]} - 1; i >= 0; i--)); do
            cp -a --remove-destination -- "${BACKUP_PATHS[$i]}" "${BACKUP_ORIGINALS[$i]}" || failed=1
            cmp -s -- "${BACKUP_PATHS[$i]}" "${BACKUP_ORIGINALS[$i]}" || failed=1
        done
        for i in "${NEW_PATHS[@]}"; do rm -f -- "$i" || failed=1; done
        restore_boot || failed=1
        for ((i=${#NEW_DIRS[@]} - 1; i >= 0; i--)); do rmdir -- "${NEW_DIRS[$i]}" 2>/dev/null || true; done
        if [ "$failed" -eq 0 ]; then
            echo "[OK] Restored configuration, firmware and boot artifacts after failure" >&2
        else
            echo "[ERR] Recovery was incomplete. Do not reboot. Recovery files: $TX" >&2
            status=1
        fi
    fi
    if [ "$PACKAGE_LOCK_OWNED" -eq 1 ] && [ "$(cat "$PACKAGE_LOCK" 2>/dev/null)" = "drmcru-$$" ]; then rm -f -- "$PACKAGE_LOCK"; fi
    if [ -n "$TX" ] && [ "$failed" -eq 0 ]; then rm -rf -- "$TX"; fi
    # pkexec reserves 126 (dialog dismissed) and 127 (not authorized).
    case "$status" in 126|127) status=1 ;; esac
    exit "$status"
}
trap finish EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

add_key() {
    [[ "$1" =~ ^[A-Za-z0-9._+-]+$ ]] || die "Unsupported Limine kernel key: $1"
    if [ -z "${KEY_SEEN[$1]:-}" ]; then KEYS+=("$1"); KEY_SEEN[$1]=1; fi
}
cmdline() {
    local output
    output="$("$ENTRY_TOOL_BIN" --get-cmdline "$1" --no-mutex --no-hooks)"
    printf '%s\n' "$output" | tail -n 1
}
canonical() { printf '%s\n' "$1" | awk -v operation=keep "$CMDLINE_AWK"; }
old_value() {
    awk -v key="$1" '
        {
            line=$0; state=sub(/^[[:space:]]*# drmcru-(map|extra)\[/,"KERNEL_CMDLINE[",line)
            if(line !~ /^[[:space:]]*KERNEL_CMDLINE\[/) next
            sub(/^[[:space:]]*KERNEL_CMDLINE\[/,"",line)
            name=line; sub(/\].*/,"",name)
            sub(/^[^]]*\][[:space:]]*/,"",line); append=state || (substr(line,1,2)=="+=")
            sub(/^\+?=[[:space:]]*/,"",line)
            if(line ~ /^".*"$/) line=substr(line,2,length(line)-2)
            values[name]=append ? values[name] " " line : line; seen[name]=1
        }
        END {
            if(key in seen) print values[key]
            else if(index(key,"fallback") && ("fallback" in seen)) print values["fallback"]
            else print values["default"]
        }
    ' "$TX/old-owned.conf"
}
old_removals() {
    awk -v key="$1" '
        /^# drmcru-remove\[/ {
            line=$0; sub(/^# drmcru-remove\[/,"",line)
            name=line; sub(/\].*/,"",name); sub(/^[^]]*\]=/,"",line)
            values[name]=line; seen[name]=1
        }
        END {
            if(key in seen) print values[key]
            else if(index(key,"fallback") && ("fallback" in seen)) print values["fallback"]
            else print values["default"]
        }
    ' "$TX/old-owned.conf"
}

check_mapping() {
    local mapping="$1" name file path other
    name="${mapping%%:*}"; file="${mapping#*:}"
    if [ "$mapping" = "$file" ]; then name=""; fi
    [[ "$file" =~ ^[A-Za-z0-9_./-]+$ ]] && [[ "$file" != /* && "/$file/" != *"/../"* && "/$file/" != *"/./"* ]] || die "Unsafe firmware mapping: $mapping"
    if [ -n "$name" ]; then
        [[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || die "Unsafe connector name: $name"
        local matches=0
        for path in "$DRM_ROOT"/card*-*/status; do
            other="${path%/status}"; other="${other##*/}"; other="${other#*-}"
            if [[ "$other" == "$name"* ]]; then
                [ "$other" = "$name" ] || die "Connector mapping $name also matches $other in Linux"
                matches=$((matches + 1))
            fi
        done
        [ "$matches" -le 1 ] || die "Connector $name exists on multiple DRM devices"
        if [ "$OPERATION" = add ] && [ "$name" = "$CONNECTOR" ]; then
            [ "$matches" -eq 1 ] || die "Connector $name is no longer available"
        fi
    fi
}

check_image() {
    local image="$1" key="$2" extract="$TX/extract" mappings mapping file actual resolved embedded
    [ -s "$image" ] || die "Image was not produced for $key"
    rm -rf -- "$extract"; mkdir -- "$extract"
    (cd "$extract"; "$LSINITCPIO_BIN" --extract "$image" >/dev/null)
    mappings="$(printf '%s\n' "${EXPECTED_CMDLINES[$key]}" | awk -v operation=mappings "$CMDLINE_AWK")"
    while IFS= read -r mapping; do
        [ -n "$mapping" ] || continue
        file="${mapping#*:}"
        actual="$extract/usr/lib/firmware/$file"
        [ -e "$actual" ] || actual="$extract/lib/firmware/$file"
        resolved="$(realpath -e -- "$actual")"
        [[ "$resolved" == "$extract/"* ]] || die "Firmware escapes extracted image: $file"
        cmp -s -- "$FIRMWARE_ROOT/$file" "$resolved" || die "Image $key contains missing or outdated firmware: $file"
    done <<< "$mappings"
    if [ "$OPERATION" = remove ] && [ "$KEEP_FIRMWARE" -eq 0 ]; then
        [ ! -e "$extract/usr/lib/firmware/edid/$EDID_NAME" ] && [ ! -e "$extract/lib/firmware/edid/$EDID_NAME" ] || die "Image $key still contains the removed EDID"
    fi
    if [ "$IMAGE_KIND" = uki ]; then
        "$OBJCOPY_BIN" --only-section=.cmdline -O binary "$image" "$TX/embedded-cmdline"
        embedded="$(tr -d '\000' < "$TX/embedded-cmdline")"
        if [ "$EMBED_CMDLINE" -eq 1 ]; then
            [ "$(canonical "$embedded")" = "${EXPECTED_CMDLINES[$key]}" ] || die "UKI command line differs for $key"
        else
            [ -z "$embedded" ] || die "Unexpected embedded UKI command line for $key"
        fi
    fi
    rm -rf -- "$extract"
}

strict_hooks() {
    local hook
    for hook in "$BOOT_HOOKS/$1.d/"*; do
        [ -f "$hook" ] && [ -x "$hook" ] && [[ "$hook" != *.disabled ]] || continue
        "$hook"
    done
}
declare -A RESOURCE_HASHES=()
check_installed_entry() {
    local key="$1" relative="$2" actual
    awk -v wanted="$relative" "$ENTRY_AWK" "$LIMINE" > "$TX/installed-entry"
    actual="$(sed -n 's/^[[:space:]]*cmdline:[[:space:]]*//p' "$TX/installed-entry" | tail -n1)"
    if [ "$IMAGE_KIND" != uki ] || [ "$EMBED_CMDLINE" -eq 0 ] || [ -n "$actual" ]; then
        [ "$(canonical "$actual")" = "${EXPECTED_CMDLINES[$key]}" ] || die "Installed boot arguments differ for $key"
    fi
}
refresh_hashes() {
    local line prefix uri relative
    while IFS= read -r line || [ -n "$line" ]; do
        if [[ "$line" =~ ^([[:space:]]*(path|kernel_path|module_path):[[:space:]]*)boot\(\):/(.*)$ ]]; then
            prefix="${BASH_REMATCH[1]}"; uri="${BASH_REMATCH[3]}"; relative="${uri%%#*}"
            if [ -n "${RESOURCE_HASHES[$relative]:-}" ]; then
                printf '%sboot():/%s#%s\n' "$prefix" "$relative" "${RESOURCE_HASHES[$relative]}"
                continue
            fi
        fi
        printf '%s\n' "$line"
    done < "$LIMINE"
}

# Validate the supported stack before touching configuration or firmware.
[ "$(id -u)" -eq 0 ] || die "This installer must run as root"
[[ "$CONNECTOR" =~ ^[A-Za-z0-9._-]+$ && "$EDID_NAME" =~ ^[A-Za-z0-9._-]+$ && "$EDID_NAME" != . && "$EDID_NAME" != .. ]] || die "Unsafe connector or EDID file name"
for tool in mkinitcpio lsinitcpio objcopy limine-entry-tool limine-mkinitcpio pacman flock b2sum realpath diff findmnt; do
    command -v "$tool" >/dev/null || die "$tool is required for verified automatic installation; use Export on this system"
done
for tool in "$MKINITCPIO_BIN" "$LSINITCPIO_BIN" "$ENTRY_TOOL_BIN" "$OBJCOPY_BIN"; do
    [ -x "$tool" ] || die "Verified installation tool is missing: $tool"
done
[ -d "$BOOT_ROOT" ] && [ ! -L "$BOOT_ROOT" ] || die "Boot root must be a regular directory"
[ -f "$MKINIT" ] && [ ! -L "$MKINIT" ] || die "/etc/mkinitcpio.conf not found or not a regular file"
[ -f "$LIMINE" ] && [ ! -L "$LIMINE" ] || die "/boot/limine.conf not found or not a regular file"
grep -qE '^[[:space:]]*cmdline:' "$LIMINE" || die "/boot/limine.conf has no cmdline entries"
[ "$OPERATION" != add ] || [ -s "$EDID_SOURCE" ] || die "EDID source does not exist: $EDID_SOURCE"
[ -f "$LIMINE_FUNCTIONS" ] || die "Limine entry-tool integration is unavailable"
for directory in "$MKINIT_DROPIN_DIR" "$LIMINE_DROPIN_DIR" "${LIMINE_DEFAULT_CONFIG%/*}" "${FIRMWARE_TARGET%/*}"; do
    [ ! -L "$directory" ] || die "Managed directory is a symlink: $directory"
done
# This is a root-owned library shipped by the supported Limine package.
source "$LIMINE_FUNCTIONS"
load_config
resolved_esp="$(realpath -e -- "$ESP_PATH")"; resolved_boot="$(realpath -e -- "$BOOT_ROOT")"
[ "$resolved_esp" = "$resolved_boot" ] || die "Automatic installation currently requires the Limine boot partition at $BOOT_ROOT"
check_boot_partition "$BOOT_ROOT"
[ -z "$(find "$BOOT_ROOT" -mindepth 1 ! -type f ! -type d -print -quit)" ] || die "Boot partition contains links or special files; use manual Export"
[ "$(findmnt -Rno TARGET "$BOOT_ROOT" | wc -l)" -eq 1 ] || die "Nested boot mounts require manual Export"
[ -s "$MACHINE_ID_FILE" ] || die "Machine ID is missing"
MACHINE_ID="$(cat "$MACHINE_ID_FILE")"
[[ "$MACHINE_ID" =~ ^[a-fA-F0-9]{32}$ ]] || die "Invalid machine ID"
UKI_PREFIX="${CUSTOM_UKI_NAME:-$MACHINE_ID}"
[[ "$UKI_PREFIX" =~ ^[a-z0-9]+$ ]] || die "Invalid custom UKI prefix"
IMAGE_KIND=initramfs; EMBED_CMDLINE=1
if [ "${ENABLE_UKI:-no}" = yes ] && is_uefi; then
    IMAGE_KIND=uki
    if is_sb_installed && is_snapper_installed; then EMBED_CMDLINE=0; fi
fi
declare -a UKI_OPTIONS=()
read -r -a requested_options <<< "${MKINITCPIO_UKI_OPTIONS:-}"
for ((i=0; i<${#requested_options[@]}; i++)); do
    option="${requested_options[$i]}"
    case "$option" in
        --no-ukify) UKI_OPTIONS+=("$option") ;;
        --splash|--uefistub|--ukiconfig)
            i=$((i+1)); [ "$i" -lt "${#requested_options[@]}" ] || die "Missing value for $option"
            UKI_OPTIONS+=("$option" "${requested_options[$i]}") ;;
        *) die "Unverified mkinitcpio option $option; use Export" ;;
    esac
done
if [ "$OPERATION" = add ]; then check_mapping "$CONNECTOR:edid/$EDID_NAME"; fi
check_mkinitcpio_dropins
mkdir -p -- "${BOOT_LOCK%/*}"
exec 201>"$BOOT_LOCK"; flock --timeout=30 201 || die "Another boot operation is running"
(set -o noclobber; printf 'drmcru-%s' "$$" > "$PACKAGE_LOCK") 2>/dev/null || die "A package transaction is running"
PACKAGE_LOCK_OWNED=1
TX="$(mktemp -d "$TRANSACTION_ROOT/drmcru-boot.XXXXXXXX")"; chmod 700 "$TX"
mkdir -- "$TX/boot"
cp -a -- "$BOOT_ROOT/." "$TX/boot/"
diff -qr --no-dereference -- "$TX/boot" "$BOOT_ROOT" >/dev/null
SNAPSHOT_READY=1

add_key default; add_key fallback
declare -A KERNEL_SEEN=()
for directory in "$MODULES_ROOT"/*; do
    [ -f "$directory/modules.builtin" ] && [ -f "$directory/vmlinuz" ] || continue
    name="$(pacman -Qqo "$directory/modules.builtin")"
    [[ "$name" =~ ^[A-Za-z0-9._+-]+$ ]] || die "Unsupported kernel package name: $name"
    [ -z "${KERNEL_SEEN[$name]:-}" ] || die "Multiple installed versions of $name require manual Export"
    KERNEL_SEEN[$name]=1
    KERNEL_NAMES+=("$name"); KERNEL_VERSIONS+=("${directory##*/}")
    add_key "$name"; add_key "$name-fallback"
done
[ "${#KERNEL_NAMES[@]}" -gt 0 ] || die "No installed kernels could be verified"
for config in "$LIMINE_VENDOR_DIR/"*.conf "$LIMINE_CONFIG" "$LIMINE_DROPIN_DIR/"*.conf "$LIMINE_DEFAULT_CONFIG"; do
    [ -f "$config" ] || continue
    config_keys="$(awk '
        /^[[:space:]]*KERNEL_CMDLINE\[/ { line=$0; sub(/^[[:space:]]*KERNEL_CMDLINE\[/,"",line); sub(/\].*/,"",line); print line }
    ' "$config")"
    while IFS= read -r key; do [ -z "$key" ] || add_key "$key"; done <<< "$config_keys"
done
for key in "${KEYS[@]}"; do
    current="$(cmdline "$key")"
    ORIGINAL_CMDLINES[$key]="$(canonical "$current")"
done

# Keep an existing bootable entry and its exact resources for recovery.
RECOVERY_RECORD="$TX/recovery-entry"
for name in "${KERNEL_NAMES[@]}"; do
    for relative in "EFI/Linux/${UKI_PREFIX}_${name}.efi" "$MACHINE_ID/$name/vmlinuz"; do
        if awk -v wanted="$relative" "$ENTRY_AWK" "$TX/boot/limine.conf" > "$RECOVERY_RECORD"; then break 2; fi
    done
done
[ -s "$RECOVERY_RECORD" ] || die "No existing kernel entry could be preserved for recovery"
if [ -e "$BOOT_ROOT/drmcru-recovery" ]; then
    [ -f "$BOOT_ROOT/drmcru-recovery/.drmcru-owned" ] || die "Recovery path is not owned by drmcru"
fi

for path in "$MKINIT" "$MKINIT_DROPIN" "$LIMINE_DROPIN" "$LIMINE_DEFAULT_CONFIG" "$FIRMWARE_TARGET"; do backup_file "$path"; done
for config in "$LIMINE_VENDOR_DIR/"*.conf "$LIMINE_CONFIG" "$LIMINE_DROPIN_DIR/"*.conf "$LIMINE_DEFAULT_CONFIG"; do
    [ ! -f "$config" ] || backup_file "$config"
done
for directory in "$MKINIT_DROPIN_DIR" "$LIMINE_DROPIN_DIR" "${FIRMWARE_TARGET%/*}" "${LIMINE_DEFAULT_CONFIG%/*}"; do
    if [ ! -d "$directory" ]; then NEW_DIRS+=("$directory"); fi
done
[ ! -f "$LIMINE_DROPIN" ] || cp -a -- "$LIMINE_DROPIN" "$TX/old-owned.conf"
touch "$TX/old-owned.conf"
MANAGED_SNAPSHOT=0
if head -n 1 "$TX/old-owned.conf" | grep -q '^# drmcru mapping state v1$'; then
    recorded="$(sed -n '2s/^# sha256=//p' "$TX/old-owned.conf")"
    actual="$(tail -n +3 "$TX/old-owned.conf" | sha256sum | cut -d ' ' -f1)"
    [ "$recorded" = "$actual" ] || die "Managed Limine configuration was edited; move custom settings to /etc/default/limine first"
    awk '!/^[[:space:]]*(KERNEL_CMDLINE\[|# drmcru-(map|extra|remove)\[)/ && NR>2' "$TX/old-owned.conf" > "$TX/base-owned.conf"
    MANAGED_SNAPSHOT=1
else
    # Legacy EDID-only defaults must disappear so the original fallback works.
    awk '!/^[[:space:]]*KERNEL_CMDLINE\[/' "$TX/old-owned.conf" > "$TX/base-owned.conf"
fi
MUTATED=1
mkdir -p -- "$LIMINE_DROPIN_DIR"
install -m 0644 -- "$TX/base-owned.conf" "$LIMINE_DROPIN"
mkdir -p -- "${LIMINE_DEFAULT_CONFIG%/*}"
touch "$LIMINE_DEFAULT_CONFIG"
awk '
    /^# BEGIN drmcru command lines$/ { if(inside || seen++) exit 1; inside=1; next }
    /^# END drmcru command lines$/ { if(!inside) exit 1; inside=0; next }
    !inside { print }
    END { if(inside) exit 1 }
' "$LIMINE_DEFAULT_CONFIG" > "$TX/default-base.conf"
awk '
    /^# BEGIN drmcru command lines$/ { inside=1; next }
    /^# END drmcru command lines$/ { inside=0; next }
    inside { print }
' "$LIMINE_DEFAULT_CONFIG" > "$TX/old-command-lines.conf"
cat "$TX/default-base.conf" > "$LIMINE_DEFAULT_CONFIG"
cp -- "$TX/base-owned.conf" "$TX/new-owned.conf"
: > "$TX/command-lines.conf"
KEEP_FIRMWARE=0; NEED_COMMAND_BLOCK=0
for key in "${KEYS[@]}"; do
    current="$(cmdline "$key")"
    base="$(canonical "$current")"
    [ -n "$base" ] || die "Effective kernel command line is empty for $key"
    inherited="$(old_value "$key")"
    extra="$(printf '%s\n' "$inherited" | awk -v operation=strip "$CMDLINE_AWK")"
    retained="$(printf '%s\n' "${ORIGINAL_CMDLINES[$key]}" | awk -v operation=strip "$CMDLINE_AWK")"
    baseline_others="$(printf '%s\n' "$base" | awk -v operation=strip "$CMDLINE_AWK")"
    previous="$(awk -v prefix="KERNEL_CMDLINE[$key]=" 'index($0,prefix)==1 { print substr($0,length(prefix)+1) }' "$TX/old-command-lines.conf")"
    previous_others="$(printf '%s\n' "$previous" | awk -v operation=strip "$CMDLINE_AWK")"
    if [ -z "$retained" ] || { [ "$MANAGED_SNAPSHOT" -eq 0 ] && [ "$retained" = "$extra" ]; } \
        || { [ -n "$previous" ] && [ "$retained" = "$previous_others" ]; }; then
        retained="$baseline_others${extra:+ $extra}"
    fi
    removals="$(old_removals "$key")"
    if [ "$OPERATION" = remove ]; then removals="${removals:+$removals,}$CONNECTOR:edid/$EDID_NAME"; fi
    removals="$(printf '%s\n' "$base" | awk -v operation=mappings "$CMDLINE_AWK" | awk -v candidates="$removals" '
        BEGIN { split(candidates, parts, ","); for(i in parts) wanted[parts[i]]=1 }
        $0 in wanted { result=result (result=="" ? "" : ",") $0 }
        END { print result }
    ')"
    expected="$(printf '%s\n%s\n' "$base" "$inherited" | awk -v operation="$OPERATION" -v connector="$CONNECTOR" -v firmware="edid/$EDID_NAME" -v suppressed="$removals" -v replace_others=1 -v retained_others="$retained" "$CMDLINE_AWK")"
    EXPECTED_CMDLINES[$key]="$expected"
    contribution="$(printf '\n%s\n' "$inherited" | awk -v operation="$OPERATION" -v connector="$CONNECTOR" -v firmware="edid/$EDID_NAME" -v suppressed="$removals" "$CMDLINE_AWK")"
    [ -z "$contribution" ] || printf '# drmcru-map[%s]= %s\n' "$key" "$contribution" >> "$TX/new-owned.conf"
    [ -z "$extra" ] || printf '# drmcru-extra[%s]=%s\n' "$key" "$extra" >> "$TX/new-owned.conf"
    [ -z "$removals" ] || printf '# drmcru-remove[%s]=%s\n' "$key" "$removals" >> "$TX/new-owned.conf"
    printf 'KERNEL_CMDLINE[%s]=%s\n' "$key" "$expected" >> "$TX/command-lines.conf"
    [ "$expected" = "$base" ] || NEED_COMMAND_BLOCK=1
    mappings="$(printf '%s\n' "$expected" | awk -v operation=mappings "$CMDLINE_AWK")"
    while IFS= read -r mapping; do
        [ -n "$mapping" ] || continue
        check_mapping "$mapping"
        [ "${mapping#*:}" != "edid/$EDID_NAME" ] || KEEP_FIRMWARE=1
    done <<< "$mappings"
done
body_hash="$(sha256sum "$TX/new-owned.conf")"
{
    echo '# drmcru mapping state v1'
    printf '# sha256=%s\n' "${body_hash%% *}"
    cat -- "$TX/new-owned.conf"
} > "$LIMINE_DROPIN"
if [ "$NEED_COMMAND_BLOCK" -eq 1 ]; then
    { echo '# BEGIN drmcru command lines'; cat "$TX/command-lines.conf"; echo '# END drmcru command lines'; } >> "$LIMINE_DEFAULT_CONFIG"
fi
for key in "${KEYS[@]}"; do
    current="$(cmdline "$key")"
    [ "$(canonical "$current")" = "${EXPECTED_CMDLINES[$key]}" ] || die "A higher-priority Limine setting overrides $key; use Export"
done
if [ "$OPERATION" = add ]; then install -D -m 0644 -- "$EDID_SOURCE" "$FIRMWARE_TARGET";
elif [ "$KEEP_FIRMWARE" -eq 0 ]; then rm -f -- "$FIRMWARE_TARGET"; fi
if [ "$OPERATION" = remove ] && [ "$KEEP_FIRMWARE" -eq 0 ]; then update_mkinitcpio_dropin remove; fi
remove_legacy_mkinitcpio_entry
# Every referenced firmware file must be available in each early initramfs.
for key in "${KEYS[@]}"; do
    mappings="$(printf '%s\n' "${EXPECTED_CMDLINES[$key]}" | awk -v operation=mappings "$CMDLINE_AWK")"
    while IFS= read -r mapping; do
        [ -n "$mapping" ] || continue
        file="${mapping#*:}"
        [ -f "$FIRMWARE_ROOT/$file" ] || die "Referenced EDID firmware is missing: $file"
        append_firmware() { local FIRMWARE_TARGET="$FIRMWARE_ROOT/$file"; update_mkinitcpio_dropin add; }
        append_firmware
    done <<< "$mappings"
done

export HOOK_CALLER=drmcru HOOK_CMDLINE="$OPERATION $CONNECTOR"
strict_hooks pre
# Build and inspect every default/fallback before replacing any boot image.
for ((k=0; k<${#KERNEL_NAMES[@]}; k++)); do
    name="${KERNEL_NAMES[$k]}"; version="${KERNEL_VERSIONS[$k]}"
    fallbacks=(default)
    if [ "${MKINITCPIO_FALLBACK:-}" = yes ] || [ "${MKINITCPIO_FALLBACK:-}" = "$name" ] \
        || [ -f "$TX/boot/$MACHINE_ID/$name/initramfs-fallback" ] \
        || [ -f "$TX/boot/EFI/Linux/${UKI_PREFIX}_${name}-fallback.efi" ]; then fallbacks+=(fallback); fi
    for variant in "${fallbacks[@]}"; do
        key="$name"; suffix=""; args=(--kernel "$version")
        if [ "$variant" = fallback ]; then key="$name-fallback"; suffix=-fallback; args+=(-S autodetect); fi
        # entry-tool retains initramfs basenames when copying regular images.
        stage_dir="$TX/stage-${#IMAGES[@]}"; mkdir -- "$stage_dir"
        stage="$stage_dir/initramfs$suffix"
        if [ "$IMAGE_KIND" = uki ]; then
            stage="$stage_dir/${UKI_PREFIX}_${key}.efi"
            if [ "$EMBED_CMDLINE" -eq 1 ]; then
                printf '%s\n' "${EXPECTED_CMDLINES[$key]}" > "$TX/cmdline"
                args+=(--cmdline "$TX/cmdline")
            else args+=(--no-cmdline); fi
            args+=(--uki "$stage" "${UKI_OPTIONS[@]}")
            destination="EFI/Linux/${UKI_PREFIX}_${key}.efi"
        else
            args+=(--no-cmdline --generate "$stage")
            destination="$MACHINE_ID/$name/initramfs$suffix"
        fi
        "$MKINITCPIO_BIN" "${args[@]}"
        check_image "$stage" "$key"
        IMAGES+=("$stage"); IMAGE_KEYS+=("$key"); IMAGE_NAMES+=("$name"); DESTINATIONS+=("$destination")
    done
done
growth=0; largest=0; recovery_size=0
for ((i=0; i<${#IMAGES[@]}; i++)); do
    size="$(stat -c %s "${IMAGES[$i]}")"; previous=0
    [ ! -f "$BOOT_ROOT/${DESTINATIONS[$i]}" ] || previous="$(stat -c %s "$BOOT_ROOT/${DESTINATIONS[$i]}")"
    [ "$size" -le "$previous" ] || growth=$((growth + size - previous))
    [ "$size" -le "$largest" ] || largest="$size"
done
if [ "$IMAGE_KIND" = initramfs ]; then
    for ((k=0; k<${#KERNEL_NAMES[@]}; k++)); do
        name="${KERNEL_NAMES[$k]}"; version="${KERNEL_VERSIONS[$k]}"
        size="$(stat -c %s "$MODULES_ROOT/$version/vmlinuz")"; previous=0
        [ ! -f "$BOOT_ROOT/$MACHINE_ID/$name/vmlinuz" ] || previous="$(stat -c %s "$BOOT_ROOT/$MACHINE_ID/$name/vmlinuz")"
        [ "$size" -le "$previous" ] || growth=$((growth + size - previous))
        [ "$size" -le "$largest" ] || largest="$size"
    done
fi
while IFS= read -r line; do
    if [[ "$line" =~ ^[[:space:]]*(path|kernel_path|module_path):[[:space:]]*boot\(\):/(.*)$ ]]; then
        relative="${BASH_REMATCH[2]}"; relative="${relative%%#*}"
        [[ "/$relative/" != *'/../'* && "/$relative/" != *'/./'* && "$relative" != /* ]] || die "Unsafe recovery resource"
        [ -f "$TX/boot/$relative" ] || die "Recovery resource is missing: $relative"
        recovery_size=$((recovery_size + $(stat -c %s "$TX/boot/$relative")))
    fi
done < "$RECOVERY_RECORD"
needed=$((growth + largest + recovery_size + 16777216))
available="$(df -B1 --output=avail "$BOOT_ROOT" | tail -n1 | tr -d ' ')"
[ "$available" -ge "$needed" ] || die "Insufficient boot space for verified images and recovery"

# Preserve one original entry, including all its original boot resources.
rm -rf -- "$BOOT_ROOT/drmcru-recovery"
mkdir -- "$BOOT_ROOT/drmcru-recovery"; touch "$BOOT_ROOT/drmcru-recovery/.drmcru-owned"
echo '/drmcru recovery (before last change)' > "$TX/recovery-menu"
resource_index=0
while IFS= read -r line; do
    [[ "$line" =~ ^[[:space:]]*/ ]] && continue
    if [[ "$line" =~ ^([[:space:]]*(path|kernel_path|module_path):[[:space:]]*)(.*)$ ]]; then
        prefix="${BASH_REMATCH[1]}"; uri="${BASH_REMATCH[3]}"; uri="${uri%%#*}"
        [[ "$uri" == boot\(\):/* && "$uri/" != *'/../'* && "$uri/" != *'/./'* ]] || die "Recovery entry has an unsupported resource: $uri"
        relative="${uri#boot():/}"; source="$TX/boot/$relative"
        [ -f "$source" ] || die "Recovery resource is missing: $relative"
        resource="drmcru-recovery/$resource_index-${source##*/}"
        cp -a -- "$source" "$BOOT_ROOT/$resource"
        hash="$(b2sum "$BOOT_ROOT/$resource")"; RESOURCE_HASHES[$resource]="${hash%% *}"
        printf '%sboot():/%s#%s\n' "$prefix" "$resource" "${hash%% *}" >> "$TX/recovery-menu"
        resource_index=$((resource_index+1))
    else printf '%s\n' "$line" >> "$TX/recovery-menu"; fi
done < "$RECOVERY_RECORD"
[ "$resource_index" -gt 0 ] || die "Recovery entry has no boot resources"

for ((i=0; i<${#IMAGES[@]}; i++)); do
    key="${IMAGE_KEYS[$i]}"; name="${IMAGE_NAMES[$i]}"
    if [ "$IMAGE_KIND" = uki ]; then
        "$ENTRY_TOOL_BIN" --add-uki "$key" "${IMAGES[$i]}" --no-mutex --no-hooks
    else
        version=""; for ((k=0; k<${#KERNEL_NAMES[@]}; k++)); do [ "${KERNEL_NAMES[$k]}" != "$name" ] || version="${KERNEL_VERSIONS[$k]}"; done
        args=(--add-kernel "$name" "$MODULES_ROOT/$version/vmlinuz" "${IMAGES[$i]}")
        [ "$name" = "$key" ] || args+=(-fallback)
        "$ENTRY_TOOL_BIN" "${args[@]}" --no-mutex --no-hooks
        cmp -s -- "$MODULES_ROOT/$version/vmlinuz" "$BOOT_ROOT/$MACHINE_ID/$name/vmlinuz" || die "Installed kernel differs for $key"
        hash="$(b2sum "$BOOT_ROOT/$MACHINE_ID/$name/vmlinuz")"
        RESOURCE_HASHES[$MACHINE_ID/$name/vmlinuz]="${hash%% *}"
    fi
    cmp -s -- "${IMAGES[$i]}" "$BOOT_ROOT/${DESTINATIONS[$i]}" || die "Installed image differs for $key"
    hash="$(b2sum "$BOOT_ROOT/${DESTINATIONS[$i]}")"
    RESOURCE_HASHES[${DESTINATIONS[$i]}]="${hash%% *}"
    check_installed_entry "$key" "${DESTINATIONS[$i]}"
done
# Match the supported backend's format migration only after all new images
# are verified. The preserved recovery entry has independent resource copies.
for name in "${KERNEL_NAMES[@]}"; do
    if [ "$IMAGE_KIND" = uki ]; then
        if [ -d "$BOOT_ROOT/$MACHINE_ID/$name" ]; then
            "$ENTRY_TOOL_BIN" --remove-kernel "$name" --no-mutex --no-hooks --quiet
        fi
    else
        for key in "$name" "$name-fallback"; do
            if [ -f "$BOOT_ROOT/EFI/Linux/${UKI_PREFIX}_${key}.efi" ]; then
                "$ENTRY_TOOL_BIN" --remove-uki "$key" --no-mutex --no-hooks --quiet
            fi
        done
    fi
done
# Replace only the previous managed recovery stanza.
awk '
    /^# BEGIN drmcru recovery$/ { inside=1; next }
    /^# END drmcru recovery$/ { inside=0; next }
    !inside { print }
    END { if(inside) exit 1 }
' "$LIMINE" > "$TX/menu"
{ cat "$TX/menu"; echo '# BEGIN drmcru recovery'; cat "$TX/recovery-menu"; echo '# END drmcru recovery'; } > "$LIMINE"
refresh_hashes > "$TX/hashed-menu"; cat "$TX/hashed-menu" > "$LIMINE"
strict_hooks post
for ((i=0; i<${#IMAGES[@]}; i++)); do
    cmp -s -- "${IMAGES[$i]}" "$BOOT_ROOT/${DESTINATIONS[$i]}" || die "Boot hook changed the verified image for ${IMAGE_KEYS[$i]}"
    check_installed_entry "${IMAGE_KEYS[$i]}" "${DESTINATIONS[$i]}"
done
for relative in "${!RESOURCE_HASHES[@]}"; do
    hash="$(b2sum "$BOOT_ROOT/$relative")"
    [ "${hash%% *}" = "${RESOURCE_HASHES[$relative]}" ] || die "Boot hook changed a verified boot resource: $relative"
done
awk '/^# BEGIN drmcru recovery$/ { inside=1; next } /^# END drmcru recovery$/ { inside=0; next } inside { print }' "$LIMINE" > "$TX/recovery-after-hooks"
cmp -s -- "$TX/recovery-menu" "$TX/recovery-after-hooks" || die "Boot hook changed the recovery entry"
refresh_hashes > "$TX/hashed-menu"
cmp -s -- "$TX/hashed-menu" "$LIMINE" || die "A boot hook changed verified resource hashes"
grep -q '^# BEGIN drmcru recovery$' "$LIMINE" || die "A boot hook removed the recovery entry"
for key in "${KEYS[@]}"; do
    current="$(cmdline "$key")"
    [ "$(canonical "$current")" = "${EXPECTED_CMDLINES[$key]}" ] || die "A boot hook changed kernel arguments for $key"
done
if [ "$OPERATION" = add ]; then check_mapping "$CONNECTOR:edid/$EDID_NAME"; fi
sync -f "$BOOT_ROOT"
COMMITTED=1
echo "[OK] All kernel images, firmware and command lines verified. Recovery entry retained."
echo "Reboot to activate the EDID changes."
