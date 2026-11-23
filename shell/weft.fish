# Weft shell integration for fish (>= 3.1).
#
#   echo 'source /ruta/a/weft.fish' >> ~/.config/fish/config.fish
#
# Emits OSC 133 A/B/C/D and OSC 7. Delete the source line to revert.

function __weft_prompt_start --on-event fish_prompt
    printf '\e]133;A\a'
end

function __weft_preexec --on-event fish_preexec
    printf '\e]133;B\a\e]133;C\a'
end

function __weft_postexec --on-event fish_postexec
    printf '\e]133;D;%s\a' $status
end

function __weft_cwd --on-variable PWD
    printf '\e]7;file://%s%s\a' (hostname) $PWD
end
