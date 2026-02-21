pub const BASH: &str = r##"# sipag completion for bash
#
# Install:
#   source <(sipag completions bash)
#   # or persist it:
#   sipag completions bash > ~/.bash_completion.d/sipag
#   echo 'source ~/.bash_completion.d/sipag' >> ~/.bashrc

_sipag_repos() {
    local conf="${SIPAG_DIR:-$HOME/.sipag}/repos.conf"
    [[ -f "$conf" ]] || return
    grep -v '^#' "$conf" | grep -v '^[[:space:]]*$' | cut -d= -f1 | tr -d ' '
}

_sipag_tasks() {
    local dir="${SIPAG_DIR:-$HOME/.sipag}"
    local subdir f base
    for subdir in running failed done; do
        [[ -d "$dir/$subdir" ]] || continue
        for f in "$dir/$subdir"/*.md; do
            [[ -f "$f" ]] || continue
            base="${f##*/}"
            echo "${base%.md}"
        done
    done
}

_sipag() {
    local cur prev subcmd i
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    subcmd=""

    local commands="start setup work drain resume merge status triage run ps logs kill add show retry repo init tui version completions help"

    # Find the subcommand (first non-flag word after "sipag")
    for (( i=1; i < COMP_CWORD; i++ )); do
        if [[ "${COMP_WORDS[i]}" != -* ]]; then
            subcmd="${COMP_WORDS[i]}"
            break
        fi
    done

    case "$subcmd" in
        completions)
            COMPREPLY=( $(compgen -W "bash zsh fish" -- "$cur") )
            return 0
            ;;
        repo)
            COMPREPLY=( $(compgen -W "add list" -- "$cur") )
            return 0
            ;;
        work|start|merge|triage)
            COMPREPLY=( $(compgen -W "$(_sipag_repos)" -- "$cur") )
            return 0
            ;;
        logs|kill|show|retry)
            COMPREPLY=( $(compgen -W "$(_sipag_tasks)" -- "$cur") )
            return 0
            ;;
        run)
            case "$prev" in
                --repo|--issue) COMPREPLY=(); return 0 ;;
                *)
                    COMPREPLY=( $(compgen -W "--repo --issue --background -b" -- "$cur") )
                    return 0
                    ;;
            esac
            ;;
        add)
            case "$prev" in
                --repo)
                    COMPREPLY=( $(compgen -W "$(_sipag_repos)" -- "$cur") )
                    return 0
                    ;;
                --priority)
                    COMPREPLY=( $(compgen -W "low medium high p0 p1 p2 p3" -- "$cur") )
                    return 0
                    ;;
                *)
                    COMPREPLY=( $(compgen -W "--repo --priority" -- "$cur") )
                    return 0
                    ;;
            esac
            ;;
    esac

    # Default: complete top-level commands
    COMPREPLY=( $(compgen -W "$commands" -- "$cur") )
}

complete -F _sipag sipag
"##;

pub const ZSH: &str = r##"#compdef sipag
# sipag completion for zsh
#
# Install:
#   sipag completions zsh > ~/.zsh/completions/_sipag
#   # Ensure the directory is in fpath (add to ~/.zshrc):
#   #   fpath=(~/.zsh/completions $fpath)
#   #   autoload -Uz compinit && compinit

_sipag_repos() {
    local conf="${SIPAG_DIR:-$HOME/.sipag}/repos.conf"
    local -a repos
    [[ -f "$conf" ]] || return
    local name url
    while IFS='=' read -r name url; do
        name="${name//[[:space:]]/}"
        [[ -z "$name" || "$name" == '#'* ]] && continue
        url="${url//[[:space:]]/}"
        repos+=("${name}:${url}")
    done < "$conf"
    (( ${#repos[@]} )) && _describe 'repo' repos
}

_sipag_tasks() {
    local dir="${SIPAG_DIR:-$HOME/.sipag}"
    local -a tasks
    local subdir f
    for subdir in running failed done; do
        [[ -d "$dir/$subdir" ]] || continue
        for f in "$dir/$subdir"/*.md; do
            [[ -f "$f" ]] || continue
            tasks+=("${${f##*/}%.md}")
        done
    done
    (( ${#tasks[@]} )) && _describe 'task' tasks
}

_sipag() {
    local state line context
    typeset -A opt_args

    _arguments -C \
        '(-h --help)'{-h,--help}'[Show help]' \
        '(-V --version)'{-V,--version}'[Print version]' \
        '1: :->command' \
        '*:: :->args' && return 0

    case $state in
        command)
            local -a commands
            commands=(
                'start:Prime a Claude Code session with board state'
                'setup:Configure sipag and Claude Code permissions'
                'work:Poll GitHub for approved issues, code in Docker'
                'drain:Signal workers to finish current batch and exit'
                'resume:Clear drain signal so workers continue polling'
                'merge:Conversational PR merge session'
                'status:Show worker state across all repos'
                'triage:Review open issues against VISION.md'
                'run:Launch a Docker sandbox for a task'
                'ps:List running and recent tasks'
                'logs:Print the log for a task'
                'kill:Kill a running container'
                'add:Queue a task'
                'show:Print task file and log'
                'retry:Move a failed task back to queue'
                'repo:Manage the repo registry'
                'init:Create ~/.sipag directories'
                'tui:Launch interactive TUI'
                'version:Print version'
                'completions:Print shell completion scripts'
                'help:Show help'
            )
            _describe 'command' commands
            ;;
        args)
            case $line[1] in
                completions)
                    local -a shells
                    shells=('bash:Bash completion script' 'zsh:Zsh completion script' 'fish:Fish completion script')
                    _describe 'shell' shells
                    ;;
                repo)
                    local -a subcmds
                    subcmds=(
                        'add:Register a repo name → URL mapping'
                        'list:List registered repos'
                    )
                    _describe 'subcommand' subcmds
                    ;;
                work|start|merge)
                    _sipag_repos
                    ;;
                triage)
                    _arguments \
                        '--dry-run[Print report only, no changes]' \
                        '--apply[Apply without confirmation]' \
                        ':repo:'
                    ;;
                logs|kill|show|retry)
                    _sipag_tasks
                    ;;
                run)
                    _arguments \
                        '--repo[Repository URL]:url:' \
                        '--issue[GitHub issue number]:issue:' \
                        '(-b --background)'{-b,--background}'[Run in background]' \
                        ':description:'
                    ;;
                add)
                    _arguments \
                        '--repo[Repository name]:repo:_sipag_repos' \
                        '--priority[Priority level]:priority:(low medium high p0 p1 p2 p3)' \
                        ':title:'
                    ;;
            esac
            ;;
    esac
}

_sipag "$@"
"##;

pub const FISH: &str = r##"# sipag completion for fish shell
#
# Install:
#   sipag completions fish > ~/.config/fish/completions/sipag.fish

# Disable file completion by default
complete -c sipag -f

# Helper: list repo names from ~/.sipag/repos.conf
function __sipag_repos
    set -l sipag_dir (set -q SIPAG_DIR; and echo $SIPAG_DIR; or echo $HOME/.sipag)
    set -l conf $sipag_dir/repos.conf
    if test -f $conf
        grep -v '^#' $conf | grep -v '^\s*$' | string replace -r '=.*' ''
    end
end

# Helper: list task IDs from running/failed/done
function __sipag_tasks
    set -l sipag_dir (set -q SIPAG_DIR; and echo $SIPAG_DIR; or echo $HOME/.sipag)
    for subdir in running failed done
        set -l dir $sipag_dir/$subdir
        if test -d $dir
            for f in $dir/*.md
                if test -f $f
                    string replace -r '\.md$' '' (basename $f)
                end
            end
        end
    end
end

# Top-level commands (shown when no subcommand has been given yet)
set -l sipag_cmds start setup work drain resume merge status triage run ps logs kill add show retry repo init tui version completions help

complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a start       -d 'Prime a Claude Code session with board state'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a setup       -d 'Configure sipag and Claude Code permissions'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a work        -d 'Poll GitHub for approved issues, code in Docker'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a drain       -d 'Signal workers to finish current batch and exit'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a resume      -d 'Clear drain signal so workers continue polling'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a merge       -d 'Conversational PR merge session'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a status      -d 'Show worker state across all repos'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a triage      -d 'Review open issues against VISION.md'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a run         -d 'Launch a Docker sandbox for a task'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a ps          -d 'List running and recent tasks'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a logs        -d 'Print the log for a task'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a kill        -d 'Kill a running container'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a add         -d 'Queue a task'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a show        -d 'Print task file and log'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a retry       -d 'Move a failed task back to queue'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a repo        -d 'Manage the repo registry'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a init        -d 'Create ~/.sipag directories'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a tui         -d 'Launch interactive TUI'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a version     -d 'Print version'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a completions -d 'Print shell completion scripts'
complete -c sipag -n "not __fish_seen_subcommand_from $sipag_cmds" -a help        -d 'Show help'

# completions subcommand: shell argument
complete -c sipag -n '__fish_seen_subcommand_from completions' -a 'bash' -d 'Bash completion script'
complete -c sipag -n '__fish_seen_subcommand_from completions' -a 'zsh'  -d 'Zsh completion script'
complete -c sipag -n '__fish_seen_subcommand_from completions' -a 'fish' -d 'Fish completion script'

# repo subcommands
complete -c sipag -n '__fish_seen_subcommand_from repo' -a 'add'  -d 'Register a repo name → URL mapping'
complete -c sipag -n '__fish_seen_subcommand_from repo' -a 'list' -d 'List registered repos'

# Dynamic repo name completion for work/start/merge/triage
complete -c sipag -n '__fish_seen_subcommand_from work start merge' -a '(__sipag_repos)'

# triage flags
complete -c sipag -n '__fish_seen_subcommand_from triage' -l dry-run -d 'Print report only, no changes'
complete -c sipag -n '__fish_seen_subcommand_from triage' -l apply   -d 'Apply without confirmation'

# run flags
complete -c sipag -n '__fish_seen_subcommand_from run' -l repo       -d 'Repository URL'
complete -c sipag -n '__fish_seen_subcommand_from run' -l issue      -d 'GitHub issue number'
complete -c sipag -n '__fish_seen_subcommand_from run' -s b -l background -d 'Run in background'

# add flags
complete -c sipag -n '__fish_seen_subcommand_from add' -l repo     -d 'Repository name' -a '(__sipag_repos)'
complete -c sipag -n '__fish_seen_subcommand_from add' -l priority -d 'Priority level'  -a 'low medium high p0 p1 p2 p3'

# Task ID completion for logs/kill/show/retry
complete -c sipag -n '__fish_seen_subcommand_from logs kill show retry' -a '(__sipag_tasks)'
"##;
