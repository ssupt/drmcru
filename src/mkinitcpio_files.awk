# Edit literal FILES arrays without evaluating the shell config. Preserve other
# assignments, comments, quoted paths, and entries that merely share a prefix.
function fail(message) {
    print "[ERR] Cannot edit mkinitcpio FILES: " message > "/dev/stderr"
    failed = 1
    exit 1
}

function finish_token(    keep) {
    keep = !(operation == "remove" && !dynamic && value == path)
    if (!dynamic && value == path) {
        found = 1
        if (append && operation == "add") {
            if (found_in_dropin)
                keep = 0
            found_in_dropin = 1
        }
    }
    if (keep)
        result = result token
    token = value = ""
    dynamic = 0
}

{
    result = ""
    start = 1
    if (!in_files && match($0, /^[[:space:]]*FILES(\+)?=\(/)) {
        in_files = 1
        saw_array = 1
        found = 0
        result = substr($0, 1, RLENGTH)
        if (append)
            sub(/FILES=/, "FILES+=", result)
        start = RLENGTH + 1
    }
    for (i = start; i <= length($0); i++) {
        character = substr($0, i, 1)
        if (!in_files) {
            result = result substr($0, i)
            break
        }
        if (!escaped && quote != "\047" &&
            (character == "`" || (character == "$" && substr($0, i + 1, 1) == "(")))
            fail("command substitutions are unsupported; use literal array entries")
        if (escaped) {
            token = token character
            value = value character
            escaped = 0
        } else if (character == "\\" && quote != "\047") {
            token = token character
            escaped = 1
        } else if (quote != "") {
            token = token character
            if (character == quote)
                quote = ""
            else {
                value = value character
                if (quote == "\"" && (character == "$" || character == "`"))
                    dynamic = 1
            }
        } else if (character == "\"" || character == "\047") {
            token = token character
            quote = character
        } else if (character == "#" && token == "") {
            result = result substr($0, i)
            break
        } else if (character ~ /[[:space:]]/ || character == ")") {
            finish_token()
            if (character == ")") {
                if (operation == "add" && !append && !found)
                    result = result " \"" path "\""
                in_files = 0
            }
            result = result character
        } else {
            token = token character
            value = value character
            if (character == "$" || character == "`")
                dynamic = 1
        }
    }
    # Multiline quoted words and substitutions need a shell parser. Fail before
    # the caller replaces the config instead of rewriting them incorrectly.
    if (quote != "" || escaped)
        fail("multiline quoted entries and continuations are unsupported")
    finish_token()
    print result
}

END {
    if (failed)
        exit 1
    if (in_files)
        fail("unterminated FILES array")
    if (operation == "add" && ((append && !found_in_dropin) || (!append && !saw_array)))
        print "FILES" (append ? "+" : "") "=(\"" path "\")"
}
