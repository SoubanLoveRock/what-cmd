# whatcmd 的 zsh 集成。source 之后，Ctrl-G 打开查询条。
# 只修改 LBUFFER，从不执行命令，也不回车。

# 候选行的字段序号（从 1 开始），与 src/output.rs 的 format_line 保持一致。
typeset -gi _WHATCMD_F_KEY=1 _WHATCMD_F_INSERT=4 _WHATCMD_F_DELETE_TAIL=5 _WHATCMD_F_DESC=6 _WHATCMD_F_DANGER=8

# 把候选行的插入文本写入 LBUFFER 光标处：先删掉 delete_tail 个字符，再接上 insert。
# 参数：$1 整行候选。
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
  # 没有可替换的尾词，且前文末尾不是空白时，补一个空格再接 insert。
  if (( tail == 0 && keep > 0 )) && [[ $head != *[[:space:]] && $insert != [[:space:]]* ]]; then
    insert=" $insert"
  fi
  LBUFFER="${head}${insert}"
}

# 把选中的 key 写入使用记录。后台运行、丢弃输出，失败不影响命令行。
# 参数：$1 整行候选。
_whatcmd_record() {
  emulate -L zsh
  local tab=$'\t'
  local -a f
  f=("${(@ps:$tab:)1}")
  local key=${f[_WHATCMD_F_KEY]}
  [[ -n $key ]] || return 0
  whatcmd record -- "$key" >/dev/null 2>&1 &!
}

# 选中危险示例时要求确认。返回 0 表示可以插入。
# 参数：$1 整行候选。
_whatcmd_confirm() {
  emulate -L zsh
  local tab=$'\t'
  local -a f
  f=("${(@ps:$tab:)1}")
  [[ ${f[_WHATCMD_F_DANGER]} == 1 ]] || return 0
  local answer
  print -n "dangerous: ${f[_WHATCMD_F_DESC]}  insert? [y/N] "
  read -r answer
  [[ $answer == [yY] ]]
}

# 从候选行中选一行。有 fzf 用 fzf，否则退化为 zsh 的 select。
# 参数：从 stdin 读候选行，stdout 输出选中的一行。
_whatcmd_pick() {
  emulate -L zsh
  local -a rows
  rows=("${(@f)$(cat)}")
  (( ${#rows} == 0 )) && return 1
  if (( $+commands[fzf] )); then
    local tab=$'\t'
    # 列表只显示 key 与描述，完整行原样返回给调用方。
    printf '%s\n' "${rows[@]}" \
      | fzf --delimiter="$tab" --with-nth=1,6 --height=40% --reverse --no-sort \
      | head -n 1
  else
    local choice
    select choice in "${rows[@]}"; do
      [[ -n $choice ]] && { print -r -- "$choice"; break }
    done
  fi
}

# Ctrl-G 入口。读取 LBUFFER 作前缀，输入查询文本，选中后插入。
whatcmd-widget() {
  emulate -L zsh
  if (( ! $+commands[whatcmd] )); then
    zle -M "whatcmd: not found in PATH"
    return 0
  fi
  local prefix=$LBUFFER
  local query
  # read-from-minibuffer 是 zsh 自带的小部件内输入函数，回车只结束这次输入，不会交给 shell 执行。
  autoload -Uz read-from-minibuffer
  read-from-minibuffer 'whatcmd> ' || { zle reset-prompt; return 0 }
  query=$REPLY
  if [[ -z $query ]]; then
    zle reset-prompt
    return 0
  fi
  local out
  out=$(whatcmd query --prefix "$prefix" --limit 10 -- "$query")
  if [[ -z $out ]]; then
    zle reset-prompt
    zle -M "whatcmd: no candidates"
    return 0
  fi
  local picked
  picked=$(print -r -- "$out" | _whatcmd_pick)
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
