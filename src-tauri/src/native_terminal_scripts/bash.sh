# NoriShell session-only Bash integration. __NORISHELL_NONCE__ is substituted
# by Core from a UUID and is never controlled by the renderer.
__norishell_nonce='__NORISHELL_NONCE__'
__norishell_capture_command=__NORISHELL_CAPTURE_COMMAND__
# Reinstallation happens only at a confirmed empty prompt.  It is nevertheless
# a new Core session, so it must never complete an earlier nonce's command.
__norishell_seq=0
__norishell_active_id=''
__norishell_history_seq=0
__norishell_debug_ready=0
__norishell_prompt_hook_active=0
__norishell_in_hook=0

__norishell_emit() {
  printf '\033]6973;NoriShell;1;%s;%s;%s;%s\a' "$__norishell_nonce" "$1" "$2" "$3"
}

__norishell_base64() {
  printf %s "$1" | base64 | tr -d '\n'
}

__norishell_start() {
  local command="$1" payload
  [[ "$command" == __norishell_* || "$command" == __norishell_bootstrap=1* ]] && return
  ((__norishell_seq++))
  __norishell_active_id=$__norishell_seq
  if [[ -z "$command" ]]; then
    payload='-'
  elif (( __norishell_capture_command )) \
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
    # Leading whitespace is a deliberate privacy opt-out.  Overlong or
    # multiline input still emits a timing pair but never enters a text OSC.
    payload='-'
  fi
  __norishell_emit start "$__norishell_active_id" "$payload"
}

__norishell_finish_prompt() {
  local status=$?
  __norishell_prompt_hook_active=1
  if [[ -n "$__norishell_active_id" ]]; then
    __norishell_emit end "$__norishell_active_id" "$status"
    __norishell_active_id=''
  fi
  __norishell_emit prompt 0 '-'
  return "$status"
}

__norishell_preexec() { __norishell_start "$1"; }
__norishell_precmd() { __norishell_finish_prompt; }

__norishell_after_prompt() {
  local status=$?
  __norishell_prompt_hook_active=0
  __norishell_debug_ready=1
  return "$status"
}

__norishell_start_from_debug() {
  local history_line history_number command=''
  if [[ "$HISTCMD" =~ ^[0-9]+$ ]] && (( HISTCMD > __norishell_history_seq )); then
    # Bash 4+ retains the current interactive history list in this command
    # substitution. Bash 3.2 does not, so it is rejected before this hook is
    # installed rather than emitting BASH_COMMAND fragments.
    history_line=$(HISTTIMEFORMAT= builtin history 1 2>/dev/null)
    history_line="${history_line#"${history_line%%[![:space:]]*}"}"
    history_number=${history_line%%[[:space:]]*}
    if [[ "$history_number" =~ ^[0-9]+$ ]] && [[ "${history_line#"$history_number"}" == '  '* ]]; then
      command=${history_line#"$history_number"}
      command=${command:2}
      __norishell_history_seq=$history_number
    fi
  fi
  # A command omitted by HISTCONTROL/HISTIGNORE deliberately has no text, but
  # it still receives a paired start/end fact for completion timing.
  __norishell_start "$command"
}

__norishell_debug() {
  (( __norishell_in_hook || __norishell_prompt_hook_active || !__norishell_debug_ready )) && return
  # The encoded Core wrapper marks itself before eval. This prevents a
  # reinstallation at an empty prompt from becoming an old nonce's command.
  if [[ "$BASH_COMMAND" == __norishell_bootstrap=1 ]]; then
    __norishell_debug_ready=0
    return
  fi
  __norishell_in_hook=1
  __norishell_debug_ready=0
  __norishell_start_from_debug
  __norishell_in_hook=0
}

__norishell_append_precmd() {
  if declare -p PROMPT_COMMAND 2>/dev/null | grep -q '^declare -a'; then
    local item
    for item in "${PROMPT_COMMAND[@]}"; do [[ "$item" == '__norishell_precmd' ]] && return; done
    PROMPT_COMMAND=(__norishell_precmd "${PROMPT_COMMAND[@]}")
  elif [[ "${PROMPT_COMMAND:-}" != *'__norishell_precmd'* ]]; then
    PROMPT_COMMAND="__norishell_precmd${PROMPT_COMMAND:+; ${PROMPT_COMMAND}}"
  fi
}

__norishell_install_debug_prompt_hooks() {
  if declare -p PROMPT_COMMAND 2>/dev/null | grep -q '^declare -a'; then
    local item has_pre=0 has_after=0
    for item in "${PROMPT_COMMAND[@]}"; do
      [[ "$item" == '__norishell_precmd' ]] && has_pre=1
      [[ "$item" == '__norishell_after_prompt' ]] && has_after=1
    done
    (( has_pre )) || PROMPT_COMMAND=(__norishell_precmd "${PROMPT_COMMAND[@]}")
    (( has_after )) || PROMPT_COMMAND+=(__norishell_after_prompt)
  elif [[ "${PROMPT_COMMAND:-}" != *'__norishell_precmd'* ]]; then
    PROMPT_COMMAND="__norishell_precmd${PROMPT_COMMAND:+; ${PROMPT_COMMAND}}; __norishell_after_prompt"
  elif [[ "${PROMPT_COMMAND:-}" != *'__norishell_after_prompt'* ]]; then
    PROMPT_COMMAND="${PROMPT_COMMAND}; __norishell_after_prompt"
  fi
}

# bash-preexec has an authoritative full-line hook. Otherwise, modern Bash can
# retrieve its just-submitted history entry from a controlled DEBUG hook. The
# older macOS Bash 3.2 loses that entry in command substitution, and an
# externally-owned DEBUG trap is never replaced; both fail closed explicitly.
if declare -p preexec_functions >/dev/null 2>&1; then
  case " ${preexec_functions[*]} " in *' __norishell_preexec '*) ;; *) preexec_functions+=(__norishell_preexec);; esac
  if declare -p precmd_functions >/dev/null 2>&1; then
    case " ${precmd_functions[*]} " in *' __norishell_precmd '*) ;; *) precmd_functions=(__norishell_precmd "${precmd_functions[@]}");; esac
  else
    __norishell_append_precmd
  fi
  __norishell_emit ready 0 '-'
elif (( BASH_VERSINFO[0] < 4 )); then
  __norishell_emit unsupported 0 'bashHistoryUnavailable'
elif [[ -n "$(trap -p DEBUG)" && "$(trap -p DEBUG)" != *'__norishell_debug'* ]]; then
  __norishell_emit unsupported 0 'debugTrapOwned'
else
  __norishell_history_seq=$HISTCMD
  trap '__norishell_debug' DEBUG
  __norishell_install_debug_prompt_hooks
  __norishell_emit ready 0 '-'
fi
