# Explicit preset configs make mkinitcpio ignore /etc/mkinitcpio.conf.d.
# Inspect assignments without sourcing the preset or running its shell code.
{
    line = $0
    sub(/^[[:space:]]*(export[[:space:]]+|readonly[[:space:]]+)?/, "", line)
    if (match(line, /^[A-Za-z_][A-Za-z0-9_]*_config\+?=/)) {
        value = substr(line, RLENGTH + 1)
        sub(/^[[:space:]]*/, "", value)
        if (value !~ /^($|#|""[[:space:]]*(#|$)|\047\047[[:space:]]*(#|$))/) {
            print "[ERR] " FILENAME ":" FNR ": explicit mkinitcpio config disables /etc/mkinitcpio.conf.d; remove the *_config assignment or use manual Export."
            blocked = 1
        }
    }
}
END { exit blocked ? 1 : 0 }
