# Weft shell integration for zsh.
#
#   echo 'source /ruta/a/weft.zsh' >> ~/.zshrc
#
# Emits OSC 133;A (prompt starts), 133;B (prompt ends), 133;C (command runs),
# 133;D;<code> (command finished) and OSC 7 (cwd). See shell/weft.bash for the
# protocol notes. Delete the source line to revert.

__weft_precmd() {
    local code=$?
    printf '\033]133;D;%s\007' "$code"
    printf '\033]7;file://%s%s\007' "${HOST:-localhost}" "$PWD"
    printf '\033]133;A\007'
}

__weft_preexec() {
    # zsh cannot mark the end of the prompt, so B and C are emitted together
    # right before the command runs.
    printf '\033]133;B\007\033]133;C\007'
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd __weft_precmd
add-zsh-hook preexec __weft_preexec
