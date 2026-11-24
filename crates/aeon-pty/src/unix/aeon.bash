# Aeon shell integration for bash, in WSL and Git Bash. Started with
# --init-file in place of ~/.bashrc, so it first loads what the shell would
# have: a login shell's files when AEON_SHELL_LOGIN is set, else ~/.bashrc.
if [ -n "$AEON_SHELL_LOGIN" ]; then
    unset AEON_SHELL_LOGIN
    [ -f /etc/profile ] && . /etc/profile
    for __aeon_file in ~/.bash_profile ~/.bash_login ~/.profile; do
        if [ -f "$__aeon_file" ]; then
            . "$__aeon_file"
            break
        fi
    done
    unset __aeon_file
elif [ -f ~/.bashrc ]; then
    . ~/.bashrc
fi

. "${BASH_SOURCE[0]%/*}/aeon-paint.sh"

# On Windows drives every folder is writable by everyone, which dircolors
# paints with a background: show them as the folders they are.
[ -n "$LS_COLORS" ] && LS_COLORS="$LS_COLORS:ow=01;34:tw=01;34"

# OSC 133 marks each command for Aeon's command blocks: A where the prompt
# starts, B where typing starts, C when the command runs, D;exit when it
# ends. OSC 9;9 reports the folder, as a Windows path, for the header.
__aeon_ran=0
__aeon_ok=1
__aeon_at_prompt=0
__aeon_last=
__aeon_history=
__aeon_tmp=${TMPDIR:-/tmp}/aeon-history-$$

# The newest history entry's number and text, read through a file rather
# than a subshell, which costs tens of milliseconds in Git Bash.
__aeon_entry() {
    local entry
    HISTTIMEFORMAT= builtin history 1 >"$__aeon_tmp" 2>/dev/null &&
        IFS= read -r entry <"$__aeon_tmp" || return 1
    entry=${entry#"${entry%%[![:space:]]*}"}
    __aeon_entry_number=${entry%% *}
    __aeon_entry_text=${entry#"$__aeon_entry_number"}
    __aeon_entry_text=${__aeon_entry_text#  }
}

__aeon_status() {
    local code=$?
    # Whatever else PROMPT_COMMAND runs is not the user's command.
    __aeon_at_prompt=0
    if [ "$__aeon_ran" = 1 ]; then
        printf '\033]133;D;%s\033\\' "$code"
        __aeon_ran=0
        # A blank line between a command's output and the next prompt, but
        # not at the top of a cleared screen.
        case $__aeon_last in clear | reset | tput) ;; *) printf '\n' ;; esac
    fi
    if [ "$code" = 0 ]; then __aeon_ok=1; else __aeon_ok=0; fi
}

__aeon_where() {
    local p=$PWD d rest
    if [ -n "$WSL_DISTRO_NAME" ]; then
        case $p in
            /mnt/[a-z] | /mnt/[a-z]/*)
                d=${p:5:1} rest=${p:6}
                __aeon_dir="${d^^}:${rest:-/}"
                ;;
            *) __aeon_dir="//wsl.localhost/$WSL_DISTRO_NAME$p" ;;
        esac
    else
        case $p in
            /[a-zA-Z] | /[a-zA-Z]/*)
                d=${p:1:1} rest=${p:2}
                __aeon_dir="${d^^}:${rest:-/}"
                ;;
            *) __aeon_dir=$(cygpath -w "$p" 2>/dev/null || printf '%s' "$p") ;;
        esac
    fi
    __aeon_dir=${__aeon_dir//\//\\}
}

# The header shows where you are, so the prompt is just the mark: a bold
# chevron, green after success and red after a failure.
__aeon_prompt() {
    __aeon_where
    printf '\033]9;9;"%s"\033\\' "$__aeon_dir"
    local color=31
    [ "$__aeon_ok" = 1 ] && color=32
    __aeon_chevron=$'\e[1;'$color$'m❯\e[0m'
    PS1='\[\033]133;A\033\\\]\[\033[1;'$color'm\]❯\[\033[0m\] \[\033]133;B\033\\\]'
    __aeon_hook
    __aeon_entry && __aeon_history=$__aeon_entry_number
    __aeon_at_prompt=1
}

# __aeon_status must run first, to see the command's own status, and
# __aeon_prompt last. Profiles rewrite PROMPT_COMMAND as they go (Kali's
# `PROMPT_COMMAND="PROMPT_COMMAND=echo"`; systemd makes it an array), so
# every prompt puts both back. Bash before 5.1 has only the one string.
__aeon_hook() {
    if ((BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1))); then
        # Kali prints a newline before each prompt; Aeon spaces them itself.
        local rest=${PROMPT_COMMAND[0]#__aeon_status}
        rest=${rest#;}
        case $rest in echo | 'PROMPT_COMMAND=echo') rest= ;; esac
        PROMPT_COMMAND[0]="__aeon_status${rest:+;$rest}"
        [[ ${PROMPT_COMMAND[${#PROMPT_COMMAND[@]} - 1]} == __aeon_prompt ]] ||
            PROMPT_COMMAND+=(__aeon_prompt)
    else
        [[ $PROMPT_COMMAND == __aeon_status* ]] ||
            PROMPT_COMMAND="__aeon_status${PROMPT_COMMAND:+;$PROMPT_COMMAND};__aeon_prompt"
    fi
}

__aeon_preexec() {
    [ "$__aeon_at_prompt" = 1 ] || return 0
    case $BASH_COMMAND in __aeon_*) return 0 ;; esac
    __aeon_at_prompt=0
    __aeon_ran=1
    # The line as typed, when it went into history (a new entry since the
    # prompt): recoloured in place.
    if __aeon_entry && [ "$__aeon_entry_number" != "$__aeon_history" ]; then
        __aeon_last=${__aeon_entry_text%% *}
        __aeon_recolor "$__aeon_entry_text"
    else
        __aeon_last=${BASH_COMMAND%% *}
    fi
    printf '\033]133;C\033\\'
}

__aeon_hook
trap '__aeon_preexec' DEBUG
