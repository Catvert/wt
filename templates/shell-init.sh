# wt shell integration — bash / zsh.
#
# `wt cd <slug>` has to change the *current* shell's directory, which no child process
# can do: the binary prints where to go, this function goes there. Everything else is
# handed straight to the binary, so the function is transparent.
#
#   eval "$(wt shell-init bash)"     # in ~/.bashrc or ~/.zshrc

wt() {
    if [ "$1" = "cd" ]; then
        shift
        # The path is the only thing on stdout; errors and questions went to stderr,
        # where the user has already seen them.
        local dir
        dir=$(command wt cd "$@") || return
        cd -- "$dir" || return
    else
        command wt "$@"
    fi
}
