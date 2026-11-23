# Weft shell integration for bash (>= 4.4).
#
#   echo 'source /ruta/a/weft.bash' >> ~/.bashrc
#
# Emits the semantic-prompt markers Weft uses to cut the session into blocks:
#   OSC 133;A  prompt starts      OSC 133;C  command runs (output starts)
#   OSC 133;B  prompt ends        OSC 133;D;<code>  command finished
# plus OSC 7 with the current working directory.
#
# These are the same markers iTerm2 and VS Code use; Weft does not scrape the
# prompt text. Removing this file from ~/.bashrc fully reverts the behaviour.

__weft_prompt_command() {
    local code=$?
    # `D` for the previous command, with its real exit code. On the very first
    # prompt Weft ignores the stray report instead of inventing a block.
    printf '\033]133;D;%s\007' "$code"
    printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$PWD"
    printf '\033]133;A\007'
}

# Keep any existing PROMPT_COMMAND, but never install ourselves twice.
case "${PROMPT_COMMAND-}" in
    *__weft_prompt_command*) ;;
    "") PROMPT_COMMAND='__weft_prompt_command' ;;
    *) PROMPT_COMMAND="__weft_prompt_command;${PROMPT_COMMAND}" ;;
esac

# PS0 is expanded immediately before a command runs, i.e. exactly at the
# prompt-end / output-start boundary. `\e` (ESC) and `\a` (BEL) are bash prompt
# escapes, and \[ \] mark them as non-printing.
PS0='\[\e]133;B\a\e]133;C\a\]'
