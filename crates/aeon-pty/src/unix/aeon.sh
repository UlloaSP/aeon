# Aeon shell integration in WSL: starts the account's own shell the way a
# login would, with Aeon's prompt marks. Run by `wsl.exe -e sh <this file>`.
dir=$(dirname "$0")
shell=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)
[ -n "$shell" ] || shell=${SHELL:-/bin/sh}
case "$shell" in
    */zsh)
        export AEON_ZDOTDIR="${ZDOTDIR:-$HOME}"
        export AEON_SHELL_DIR="$dir"
        export ZDOTDIR="$dir/zsh"
        exec "$shell" -l
        ;;
    */bash)
        export AEON_SHELL_LOGIN=1
        exec "$shell" --init-file "$dir/aeon.bash" -i
        ;;
    *)
        exec "$shell" -l
        ;;
esac
