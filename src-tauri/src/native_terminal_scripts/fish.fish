# NoriShell session-only Fish integration.
set -g __norishell_nonce '__NORISHELL_NONCE__'
set -g __norishell_capture_command __NORISHELL_CAPTURE_COMMAND__
# Every install is a new Core capture session; do not retain a previous nonce's
# active command or sequence when the user re-enables integration.
set -g __norishell_seq 0
set -g __norishell_active_id ''

function __norishell_emit
  printf '\e]6973;NoriShell;1;%s;%s;%s;%s\a' $__norishell_nonce $argv[1] $argv[2] $argv[3]
end

function __norishell_start --on-event fish_preexec
  set -l command $argv[1]
  set -l payload '-'
  if test -z "$command"; or string match -q '__norishell_*' -- "$command"
    return
  end
  set -g __norishell_seq (math $__norishell_seq + 1)
  set -g __norishell_active_id $__norishell_seq
  if test "$__norishell_capture_command" = 1; and not string match -q '[[:space:]]*' -- "$command"
    set -l bytes (string length --bytes -- "$command")
    if test $bytes -le 16384; and not string match -q '*\n*' -- "$command"; and not string match -q '*\r*' -- "$command"
      set payload (string collect -- $command | base64 | string replace -a '\n' '')
    end
  end
  __norishell_emit start $__norishell_active_id $payload
end

function __norishell_finish --on-event fish_postexec
  set -l exit_code $status
  if test -n "$__norishell_active_id"
    __norishell_emit end $__norishell_active_id $exit_code
    set -g __norishell_active_id ''
  end
end

function __norishell_prompt --on-event fish_prompt
  __norishell_emit prompt 0 '-'
end

__norishell_emit ready 0 '-'
