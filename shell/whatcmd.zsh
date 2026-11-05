# whatcmd zsh integration. After sourcing, Ctrl-G opens an input bar below the prompt.
# Only modifies LBUFFER. Never executes a command and never presses Enter.
#
# All UI is drawn by fzf. Colors come from the poster palette:
# orange for the prompt, teal for the selected row, mustard for anchors and matched words,
# brick for danger and empty results, olive for secondary info.

# Field indexes of a candidate line (1-based), matching format_line in src/output.rs.
typeset -gi _WHATCMD_F_KEY=1 _WHATCMD_F_INSERT=4 _WHATCMD_F_DELETE_TAIL=5 _WHATCMD_F_DESC=6 _WHATCMD_F_DANGER=8

# Palette. The gray description is not one of the five accent colors; it sits between the text and background colors.
typeset -g _WHATCMD_C_BG='#2b2420' _WHATCMD_C_FG='#efe6d2' _WHATCMD_C_ORANGE='#d9643a'
typeset -g _WHATCMD_C_MUSTARD='#e0a83a' _WHATCMD_C_BRICK='#a5432f' _WHATCMD_C_OLIVE='#7d8a4a'
typeset -g _WHATCMD_C_TEAL='#3f7f7a' _WHATCMD_C_GRAY='#8a7d6b'

typeset -g _WHATCMD_COLORS="bg:${_WHATCMD_C_BG},fg:${_WHATCMD_C_FG},bg+:${_WHATCMD_C_TEAL},fg+:${_WHATCMD_C_FG},hl:${_WHATCMD_C_MUSTARD},hl+:${_WHATCMD_C_MUSTARD},prompt:${_WHATCMD_C_ORANGE},pointer:${_WHATCMD_C_TEAL},header:${_WHATCMD_C_MUSTARD},info:${_WHATCMD_C_OLIVE},border:${_WHATCMD_C_GRAY}"

# Shared list options: sharp border, no sorting, full-row highlight, no pointer or marker glyphs, no outline.
typeset -ga _WHATCMD_FZF=(
  --ansi --reverse --no-sort --border=sharp --pointer=' ' --marker=' '
  --delimiter=$'\t' --with-nth=1 --height=40% --color="$_WHATCMD_COLORS"
)

# Convert a hex color to a 24-bit foreground escape sequence. Argument: $1 color, e.g. #d9643a.
_whatcmd_sgr() {
  emulate -L zsh
  local h=${1#\#}
  printf '\e[38;2;%d;%d;%dm' $(( 0x${h[1,2]} )) $(( 0x${h[3,4]} )) $(( 0x${h[5,6]} ))
}

# Build one fzf display row: fixed-width path on the left, gray description on the right, brick marker in front for dangerous items.
# The raw candidate line follows a tab and is restored by _whatcmd_pick.
# Arguments: $1 key, $2 description, $3 danger flag (0/1), $4 raw candidate line.
_whatcmd_row() {
  emulate -L zsh
  local key=$1 desc=$2 danger=$3 raw=$4
  local esc=$'\e' tab=$'\t' padded mark='  '
  printf -v padded '%-34s' "$key"
  [[ $danger == 1 ]] && mark="$(_whatcmd_sgr $_WHATCMD_C_BRICK)!${esc}[39m "
  print -r -- "${mark}$(_whatcmd_sgr $_WHATCMD_C_FG)${padded}$(_whatcmd_sgr $_WHATCMD_C_GRAY)${desc}${esc}[0m${tab}${raw}"
}

# Write the chosen insert text at the cursor in LBUFFER: delete delete_tail characters first, then append insert.
# Argument: $1 full candidate line.
_whatcmd_apply() {
  emulate -L zsh
  local tab=$'\t'
  local -a f
  f=("${(@ps:$tab:)1}")
  local insert=${f[_WHATCMD_F_INSERT]}
  local -i tail=${f[_WHATCMD_F_DELETE_TAIL]}
  local -i keep=$(( ${#LBUFFER} - tail ))
  (( keep < 0 )) && keep=0
  local head=${LBUFFER[1,keep]}
  # No partial word to replace and the text before the cursor does not end in whitespace: add a space before insert.
  if (( tail == 0 && keep > 0 )) && [[ $head != *[[:space:]] && $insert != [[:space:]]* ]]; then
    insert=" $insert"
  fi
  LBUFFER="${head}${insert}"
}

# Record the selected key in the usage log. Runs in the background, output discarded, failures do not affect the command line.
# Argument: $1 full candidate line.
_whatcmd_record() {
  emulate -L zsh
  local tab=$'\t'
  local -a f
  f=("${(@ps:$tab:)1}")
  local key=${f[_WHATCMD_F_KEY]}
  [[ -n $key ]] || return 0
  whatcmd record -- "$key" >/dev/null 2>&1 &!
}

# Ask for confirmation when a dangerous example is selected. Returns 0 if insertion is allowed.
# Inside a widget stdin is /dev/null, so the answer must be read from /dev/tty.
# Argument: $1 full candidate line.
_whatcmd_confirm() {
  emulate -L zsh
  local tab=$'\t'
  local -a f
  f=("${(@ps:$tab:)1}")
  [[ ${f[_WHATCMD_F_DANGER]} == 1 ]] || return 0
  local answer
  print -n "dangerous: ${f[_WHATCMD_F_DESC]}  insert? [y/N] " >/dev/tty
  read -r answer </dev/tty
  [[ $answer == [yY] ]]
}

# Input bar: fzf has no candidates here, it only collects the query with --print-query. Esc prints nothing.
# Argument: $1 header (anchor and scope). Writes the query text to stdout.
_whatcmd_input() {
  emulate -L zsh
  local header=$1
  fzf --print-query --prompt='> ' --header="$header" --height=4 --reverse --no-info \
    --border=sharp --color="$_WHATCMD_COLORS" --pointer=' ' </dev/null 2>/dev/null | head -n 1
}

# Candidate list. Argument: $1 header; stdin holds candidate lines. Writes the selected raw candidate line to stdout, empty on Esc.
_whatcmd_pick() {
  emulate -L zsh
  local header=$1 tab=$'\t' picked
  local -a rows shown
  rows=("${(@f)$(cat)}")
  (( ${#rows} == 0 )) && return 1
  local row
  for row in "${rows[@]}"; do
    local -a f
    f=("${(@ps:$tab:)row}")
    shown+=("$(_whatcmd_row "${f[_WHATCMD_F_KEY]}" "${f[_WHATCMD_F_DESC]}" "${f[_WHATCMD_F_DANGER]}" "$row")")
  done
  picked=$(printf '%s\n' "${shown[@]}" | fzf "${_WHATCMD_FZF[@]}" --header="$header")
  [[ -n $picked ]] || return 1
  # fzf returns the whole row; strip the display part before the first tab to recover the raw candidate line.
  print -r -- "${picked#*"$tab"}"
}

# Function menu, same list style as the candidate list. Argument: $1 filter text (without /).
_whatcmd_menu() {
  emulate -L zsh
  local tab=$'\t' choice
  local -a rows=(
    "$(_whatcmd_row reindex 'Rebuild the index; runs in the foreground and shows progress' 0 reindex)"
  )
  choice=$(printf '%s\n' "${rows[@]}" | fzf "${_WHATCMD_FZF[@]}" --query="$1" --header='Menu' --height=6)
  choice=${choice#*"$tab"}
  case $choice in
    reindex)
      zle -I
      whatcmd reindex
      ;;
  esac
}

# Ctrl-G entry point. Reads LBUFFER as the anchor, collects the query, inserts the selection.
whatcmd-widget() {
  emulate -L zsh
  if (( ! $+commands[whatcmd] )); then
    zle -M "whatcmd: not found in PATH"
    return 0
  fi
  if (( ! $+commands[fzf] )); then
    zle -M "whatcmd: fzf not found in PATH"
    return 0
  fi

  local prefix=$LBUFFER
  local -a words
  words=(${(z)prefix})
  local header='Scope: all commands'
  if (( ${#words} )); then
    header="Anchor ${prefix}   Scope: subcommands and options of ${words[1]}"
  fi

  local query
  # _whatcmd_input already keeps only the first line; a subscript here would index characters, not lines.
  query=$(_whatcmd_input "$header")
  if [[ -z $query ]]; then
    zle reset-prompt
    return 0
  fi

  # A leading / with no spaces opens the function menu; anything else is a natural-language query.
  if [[ $query == /* && $query != *[[:space:]]* ]]; then
    _whatcmd_menu "${query#/}"
    zle reset-prompt
    return 0
  fi

  local out rc
  out=$(whatcmd query --prefix "$prefix" --limit 10 -- "$query" 2>/dev/null)
  rc=$?
  if [[ -z $out ]]; then
    zle reset-prompt
    if (( rc == 0 || rc == 1 )); then
      zle -M "whatcmd: no candidates"
    else
      zle -M "whatcmd: query failed (exit ${rc})"
    fi
    return 0
  fi

  local picked
  picked=$(print -r -- "$out" | _whatcmd_pick "$header")
  if [[ -z $picked ]]; then
    zle reset-prompt
    return 0
  fi
  if ! _whatcmd_confirm "$picked"; then
    zle reset-prompt
    return 0
  fi
  _whatcmd_apply "$picked"
  _whatcmd_record "$picked"
  zle reset-prompt
}

zle -N whatcmd-widget
bindkey '^G' whatcmd-widget
