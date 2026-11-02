# Tokenize without eval. Keep quoting/escaping in non-EDID arguments intact.
function mapping(value, inherited,    colon,key) {
    if (value == "") return
    colon = index(value, ":")
    key = colon ? substr(value, 1, colon - 1) : ""
    n_removed = split(suppressed, removed, ",")
    for (r = 1; r <= n_removed; r++)
        if (removed[r] == value && !(operation == "add" && key == connector)) return
    if (inherited && operation == "remove" && key == connector) return
    if (operation == "remove" && key == connector && substr(value, colon + 1) == firmware) return
    if (!(key in present)) { order[++count] = key; present[key] = 1 }
    values[key] = value
}
function token(raw, inherited,    normalized,value,n,parts,i,key,seen) {
    normalized = raw
    if (normalized ~ /^".*"$/ || normalized ~ /^\047.*\047$/)
        normalized = substr(normalized, 2, length(normalized) - 2)
    if (index(normalized, "drm.edid_firmware=") == 1) {
        value = substr(normalized, 19)
        if (value ~ /^".*"$/ || value ~ /^\047.*\047$/)
            value = substr(value, 2, length(value) - 2)
        n = split(value, parts, ",")
        # Linux uses the first matching connector within one comma list.
        for (i = 1; i <= n; i++) {
            key = parts[i]; sub(/:.*/, "", key)
            if (index(parts[i], ":") == 0) key = ""
            if (key == "" || !(key in seen)) mapping(parts[i], inherited)
            seen[key] = 1
        }
    } else if (!inherited) {
        others = others (others == "" ? "" : " ") raw
    }
}
{
    word = ""; quote = ""; escaped = 0
    for (i = 1; i <= length($0); i++) {
        c = substr($0, i, 1)
        if (!escaped && quote == "" && c ~ /[[:space:]]/) {
            if (word != "") token(word, NR > 1)
            word = ""
            continue
        }
        word = word c
        if (escaped) escaped = 0
        else if (c == "\\" && quote != "\047") escaped = 1
        else if (quote != "" && c == quote) quote = ""
        else if (quote == "" && (c == "\047" || c == "\"")) quote = c
    }
    if (quote != "" || escaped) { print "[ERR] Unclosed kernel command-line quoting" > "/dev/stderr"; failed = 1; exit 1 }
    if (word != "") token(word, NR > 1)
}
END {
    if (failed) exit 1
    if (operation == "add") mapping(connector ":" firmware, 0)
    combined = ""
    for (j = 1; j <= count; j++) {
        value = values[order[j]]
        if (operation == "mappings") print value
        combined = combined (combined == "" ? "" : ",") value
    }
    if (operation != "mappings") {
        if (replace_others) others = retained_others
        if (operation == "strip") print others
        else print others (combined == "" ? "" : (others == "" ? "" : " ") "drm.edid_firmware=" combined)
    }
}
