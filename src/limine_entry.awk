# Select an existing menu entry by its boot-relative kernel/UKI path.
function emit() {
    if (selected && !done) { print heading; printf "%s", body; done = 1 }
}
/^# (BEGIN|END) drmcru recovery$/ { next }
/^[[:space:]]*\// { emit(); heading = $0; body = ""; selected = 0; next }
{
    body = body $0 "\n"
    line = $0
    sub(/^[[:space:]]*/, "", line)
    if (line ~ /^(path|kernel_path|module_path):/) {
        sub(/^[^:]*:[[:space:]]*/, "", line)
        sub(/#.*/, "", line)
        sub(/[[:space:]]*$/, "", line)
        if (line == "boot():/" wanted) selected = 1
    }
}
END { emit(); exit done ? 0 : 1 }
