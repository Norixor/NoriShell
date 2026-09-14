# NoriShell session-only Zsh integration.
typeset -g __norishell_nonce='__NORISHELL_NONCE__'
typeset -g __norishell_capture_command=__NORISHELL_CAPTURE_COMMAND__
# Every install receives a new Core nonce and must reset its pending command.
typeset -g __norishell_seq=0
typeset -g __norishell_active_id=''

function __norishell_emit() {
  print -rn -- $'\e]6973;NoriShell;1;'"$__norishell_nonce;$1;$2;$3"$'\a'
}

function __norishell_base64() {
  print -rn -- "$1" | base64 | tr -d '\n'
}

function __norishell_preexec() {
  local command="$1" payload
  [[ -z "$command" || "$command" == __norishell_* ]] && return
  (( __norishell_seq++ ))
  __norishell_active_id=$__norishell_seq
  if (( __norishell_capture_command )) \
    && [[ "$command" != [[:space:]]* ]] \
    && [[ "$command" != *$'\n'* ]] \
    && [[ "$command" != *$'\r'* ]]; then
    local LC_ALL=C
    if (( ${#command} <= 16384 )); then
      payload=$(__norishell_base64 "$command")
    else
      payload='-'
    fi
  else
    payload='-'
  fi
  __norishell_emit start "$__norishell_active_id" "$payload"
}

function __norishell_precmd() {
  # `status` is a readonly special parameter in zsh. Keep the pre-command
  # exit code in a NoriShell-owned name so the hook works in a clean zsh too.
  local __norishell_exit_status=$?
  if [[ -n "$__norishell_active_id" ]]; then
    __norishell_emit end "$__norishell_active_id" "$__norishell_exit_status"
    __norishell_active_id=''
  fi
  __norishell_emit prompt 0 '-'
  return "$__norishell_exit_status"
}

typeset -ga preexec_functions precmd_functions
(( ${preexec_functions[(Ie)__norishell_preexec]} )) || preexec_functions+=(__norishell_preexec)
(( ${precmd_functions[(Ie)__norishell_precmd]} )) || precmd_functions=(__norishell_precmd "${precmd_functions[@]}")
__norishell_emit ready 0 '-'
