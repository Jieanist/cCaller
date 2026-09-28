#!/usr/bin/env bash
# memory.sh — agent_lookup 记忆系统工具
#   gen           扫描 decisions/incidents frontmatter → CATALOG.md
#   check         体检（必填字段 / 状态枚举 / 命名 / 尺寸 cap / hook 安装 / 自测）
#   install-hooks 安装版本化 pre-commit hook（core.hooksPath）
# 无依赖：grep / sed / awk / find。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# agent_lookup 相对 git 仓库根的路径：内嵌于 cCaller 时为 _agent/agent_lookup，
# 独立仓库时为 agent_lookup。core.hooksPath 以仓库根为基准解释相对路径。
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -n "$REPO_ROOT" ] && [ "$ROOT" != "$REPO_ROOT" ]; then
  AGENT_REL="${ROOT#"$REPO_ROOT"/}"
else
  AGENT_REL="agent_lookup"
fi
HOOKS_PATH="$AGENT_REL/tools/hooks"

RESIDENT="INDEX.md project.md status.md short_term.md environment.md errata.md"
CAP=12288          # 12KB 硬上限
WARN=8192          # 8KB 警戒线
DOMAINS=" ffi config validate expand defuse run plan report assertion cli env tool "

# 提取文件 frontmatter 中某字段的值（不含键名）
field() { # $1=key  $2=file
  sed -n '/^---$/,/^---$/p' "$2" | sed -n "s/^$1: //p" | head -n1
}

gen() {
  {
    echo "# CATALOG（生成物，勿手改；R0 开工先跑 memory.sh gen 重建）"
    echo
    echo "## 现行法律（active 决策 + open/workaround incident）"
    echo
    for f in decisions/*.md incidents/*.md; do
      [ -e "$f" ] || continue
      st=$(field status "$f"); t=$(awk 'NR==1{sub(/^# /,"");print;exit}' "$f")
      case "$st" in
        active|open|workaround) printf -- "- [%s] %s — %s\n" "$st" "$f" "$t" ;;
      esac
    done
    echo
    echo "## 全量清单"
    echo
    for f in decisions/*.md incidents/*.md; do
      [ -e "$f" ] || continue
      d=$(field domain "$f"); st=$(field status "$f")
      printf -- "- %-56s [%s] [%s]\n" "$f" "${d:-?}" "${st:-?}"
    done
  } > CATALOG.md
  echo "gen: CATALOG.md 已重建（$(ls decisions/*.md incidents/*.md 2>/dev/null | wc -l) 条）"
}

check() {
  local err=0 n=0

  # 1) 必填字段 + 状态枚举 + 受控域
  for f in decisions/*.md incidents/*.md; do
    [ -e "$f" ] || continue
    n=$((n+1))
    local kind=decision; case "$f" in incidents/*) kind=incident ;; esac
    for k in date domain status scope; do
      [ -n "$(field "$k" "$f")" ] || { echo "缺必填字段 $k: $f"; err=1; }
    done
    local st; st=$(field status "$f")
    if [ "$kind" = decision ]; then
      [ -n "$(field overrule_if "$f")" ] || { echo "decision 缺 overrule_if: $f"; err=1; }
      case "$st" in draft|active|superseded|rejected) ;; *) echo "非法 status '$st'（decision）: $f"; err=1 ;; esac
    else
      [ -n "$(field symptoms "$f")" ] || { echo "incident 缺 symptoms: $f"; err=1; }
      case "$st" in draft|open|workaround|resolved|rejected) ;; *) echo "非法 status '$st'（incident）: $f"; err=1 ;; esac
    fi
    local d; d=$(field domain "$f")
    echo "$DOMAINS" | grep -q " $d " || { echo "域 '$d' 不在受控词表: $f"; err=1; }
  done

  # 2) 尺寸 cap
  local size; size=$(cat $RESIDENT 2>/dev/null | wc -c)
  if [ "$size" -gt "$CAP" ]; then echo "常驻合计 ${size}B > ${CAP}B，须瘦身"; err=1
  elif [ "$size" -gt "$WARN" ]; then echo "常驻合计 ${size}B 越过警戒线 ${WARN}B"; fi

  # 3) 命名规范
  for f in decisions/*.md incidents/*.md; do
    [ -e "$f" ] || continue
    basename "$f" | grep -Eq '^[0-9]{4}-[0-9]{2}-[0-9]{2}-[a-z0-9-]+\.md$' \
      || { echo "命名须 YYYY-MM-DD-<域>-<症状>.md: $f"; err=1; }
  done

  # 4) hook 是否版本化安装
  local hp; hp=$(git config core.hooksPath 2>/dev/null || true)
  [ "$hp" = "$HOOKS_PATH" ] || { echo "警告: core.hooksPath 未指向 $HOOKS_PATH（跑 memory.sh install-hooks）"; err=1; }

  # 5) 自测断言（frontmatter 解析）
  local t; t=$(printf -- '---\nstatus: draft\n---\n' | sed -n '/^---$/,/^---$/p' | sed -n 's/^status: //p' | head -n1)
  [ "$t" = "draft" ] || { echo "自测失败: frontmatter 解析"; err=1; }

  [ "$err" -eq 0 ] && echo "check: 全绿（$n 条 decisions/incidents）" || { echo "check: 有 $err 处问题"; exit 1; }
}

install-hooks() {
  git config core.hooksPath "$HOOKS_PATH"
  chmod +x tools/hooks/pre-commit
  echo "已安装 core.hooksPath=$HOOKS_PATH（版本化 hook，换机重跑本命令即可）"
}

case "${1:-}" in
  gen) gen ;;
  check) check ;;
  install-hooks) install-hooks ;;
  *) echo "用法: memory.sh {gen|check|install-hooks}"; exit 2 ;;
esac
