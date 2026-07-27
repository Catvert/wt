# wt shell integration — fish.
#
# `wt cd <slug>` has to change the *current* shell's directory, which no child process
# can do: the binary prints where to go, this function goes there. Everything else is
# handed straight to the binary, so the function is transparent.
#
#   wt shell-init fish > ~/.config/fish/functions/wt.fish

function wt --wraps wt --description "git worktree manager"
    if test "$argv[1]" = "cd"
        set -e argv[1]
        # The path is the only thing on stdout; errors and questions went to stderr,
        # where the user has already seen them. Nothing printed means it failed.
        set -l dir (command wt cd $argv)
        if test -n "$dir"
            cd $dir
        end
    else
        command wt $argv
    end
end
