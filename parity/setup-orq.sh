#!/bin/sh
# Monta a worktree de fake-a e ajusta o registro. Variantes por ambiente (os wrappers setup-orq-*.sh exportam):
#   ORQ_STATE=parked|working|done   ORQ_TREE=misto|limpa|commits|dirty   ORQ_HANDOFF=0|1   ORQ_SECRET=nome|commit|handoff
#   ORQ_IGNORE_REPORTS=1  ORQ_PLAN_OUTRO=1  ORQ_OUT_EXISTS=1
set -eu
state=${ORQ_STATE:-parked}; tree_kind=${ORQ_TREE:-misto}; handoff=${ORQ_HANDOFF:-0}
secret=${ORQ_SECRET:-}; export GIT_CONFIG_GLOBAL=/dev/null
reports=.design-sync-reports
# marcador de segredo montado em partes: o literal não pode morar no repo
value="Hds""!""abcdef1234"
g() { git -c user.name=t -c user.email=t@example.test -c commit.gpgsign=false "$@"; }
at() { GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1"; export GIT_AUTHOR_DATE GIT_COMMITTER_DATE; }

tree=$HOME/trees/fake-a
mkdir -p "$tree"; cd "$tree"
g init -q -b main
printf 'um\n' > a.txt
printf 'ignorado.log\n' > .gitignore
[ "${ORQ_IGNORE_REPORTS:-}" = 1 ] && printf '%s/\n' "$reports" >> .gitignore
g add -A; at 2026-11-01T10:00:00Z; g commit -q -m base
mkdir -p "$reports"; printf '# brief\n' > "$reports/brief.md"
[ "$handoff" = 1 ] && printf 'STATUS: PARKED — pausa\nfeito: tudo\n' > "$reports/handoff.md"
case $tree_kind in
  commits|misto)
    g checkout -q -b feat/y; printf 'dois\n' > b.txt; g add -A; at 2026-11-02T10:00:00Z; g commit -q -m "feat: b" ;;
esac
case $tree_kind in
  dirty|misto) printf 'novo\n' > solto.txt ;;
esac
[ "$tree_kind" = misto ] && printf 'um\nmais\n' > a.txt
case $secret in
  nome) printf 'TOKEN=%s\n' "$value" > .env; mkdir -p docs; printf 'usuario: x\n' > docs/credenciais.md; printf 'TOKEN=<token>\n' > .env.example ;;
  commit) printf 'senha=%s\n' "$value" > config.txt; g add config.txt; at 2026-11-03T10:00:00Z; g commit -q -m "feat: config" ;;
  handoff) printf 'STATUS: PARKED — x\n%s\n' "$value" > "$reports/handoff.md" ;;
esac

cd "$HOME"
orch=work/_skill/orchestration
if [ "$tree_kind" = limpa ]; then
  sed -i 's/"branch":"feat\/y"/"branch":"main"/' "$orch/agents/fake-a.json"
fi
sed -i "s/\"state\":\"parked\"/\"state\":\"$state\"/" "$orch/agents/fake-a.json"
if [ "${ORQ_PLAN_OUTRO:-}" = 1 ]; then
  printf '# Plano demo\n\n## Estado atual\n- x\n\n## Unidades\n| fake-a | impl |\n| outro | impl |\n' > .claude/hades/plans/demo.md
fi
# O lado TS roda em .../ts/home e o Rust em .../rust/home: 2 caracteres a mais por ocorrência do HOME. Completa o
# lado TS com espaço no fim do JSON, para o tamanho das partes (impresso no --dry-run) ser o mesmo.
if [ "${SIDE:-}" = ts ]; then
  for f in "$orch/plan.json" "$orch"/agents/*.json; do
    n=$(grep -o "$HOME" "$f" | wc -l)
    [ "$n" -gt 0 ] && printf '%*s' $((2 * n)) '' >> "$f"
  done
fi
[ "${ORQ_OUT_EXISTS:-}" = 1 ] && { mkdir -p "$HOME/out"; printf 'x' > "$HOME/out/p.caronte"; }
:
