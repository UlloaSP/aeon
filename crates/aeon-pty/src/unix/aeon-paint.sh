# Aeon: a command line in colour by part, as PowerShell's is: commands bold
# yellow, arguments bold white, options gray, strings lavender, variables
# green, operators gray. Sourced by Aeon's bash and zsh integrations.

# Sets __aeon_painted to `$1` in colour. No subshell: it runs on every Enter.
__aeon_paint() {
    [ -n "${ZSH_VERSION-}" ] && setopt local_options bash_rematch ksh_arrays
    local s=$1 tok first=1 e=$'\e'
    local r_space='^[[:space:]]+'
    local r_op='^(\|\||&&|;;|[|;&()])'
    local r_redirect='^[0-9]*(>>|>|<<|<)&?[0-9-]*'
    local r_squote="^'[^']*'?"
    local r_dquote='^"([^"\\]|\\.)*"?'
    local r_var='^\$(\{[^}]*\}?|[A-Za-z_][A-Za-z0-9_]*|[0-9?$!#@*-])'
    local r_word='^[^[:space:]|;&()<>'"'"'"$]+'
    __aeon_painted=
    while [ -n "$s" ]; do
        if [[ $s =~ $r_space ]]; then
            tok=${BASH_REMATCH[0]}
            __aeon_painted+=$tok
        elif [[ $s =~ $r_op ]]; then
            tok=${BASH_REMATCH[0]}
            __aeon_painted+="$e[38;5;245m$tok$e[0m"
            first=1
        elif [[ $s =~ $r_redirect ]]; then
            tok=${BASH_REMATCH[0]}
            __aeon_painted+="$e[38;5;245m$tok$e[0m"
        elif [[ $s =~ $r_squote ]] || [[ $s =~ $r_dquote ]]; then
            tok=${BASH_REMATCH[0]}
            __aeon_painted+="$e[1;95m$tok$e[0m"
            first=0
        elif [[ $s =~ $r_var ]]; then
            tok=${BASH_REMATCH[0]}
            __aeon_painted+="$e[1;92m$tok$e[0m"
            first=0
        elif [[ $s =~ $r_word ]]; then
            tok=${BASH_REMATCH[0]}
            if [ "$first" = 1 ] && [[ $tok != *=* ]]; then
                __aeon_painted+="$e[1;93m$tok$e[0m"
                first=0
            elif [[ $first = 0 && $tok == -* ]]; then
                __aeon_painted+="$e[1;38;5;245m$tok$e[0m"
            else
                __aeon_painted+="$e[1;97m$tok$e[0m"
            fi
        else
            tok=${s:0:1}
            __aeon_painted+=$tok
        fi
        s=${s:${#tok}}
    done
}

# Redraws the line just entered, in colour, over what the shell echoed after
# the two-cell prompt. Lines of more than one row of input are left alone.
__aeon_recolor() {
    local line=$1 cols=${COLUMNS:-80}
    case $line in '' | *$'\n'*) return ;; esac
    __aeon_paint "$line"
    printf '\033[%dF\033[0J%s %s\n' $(((2 + ${#line} + cols - 1) / cols)) \
        "$__aeon_chevron" "$__aeon_painted"
}
