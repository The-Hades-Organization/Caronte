#!/bin/sh
# Monta @HOME@/clone: o repo de fake-a só com a base (sem a branch feat/y), na mesma data da fixture orq (sha estável).
# Variantes (os wrappers setup-unpack-*.sh exportam UNPACK_VAR): plano-diferente, worktree-existe, branch-existe,
# registro-existe, sem-clone, base-fora, clone-nao-repo, truncado, nao-pacote, log2n, header.
set -eu
export GIT_CONFIG_GLOBAL=/dev/null
var=${UNPACK_VAR:-ok}
g() { git -c user.name=t -c user.email=t@example.test -c commit.gpgsign=false "$@"; }
pacote=$HOME/pacotes/ts.caronte

case $var in
  sem-clone) ;;
  clone-nao-repo) mkdir -p "$HOME/clone"; printf 'x\n' > "$HOME/clone/a.txt" ;;
  *)
    mkdir -p "$HOME/clone"; cd "$HOME/clone"
    g init -q -b main
    if [ "$var" = base-fora ]; then printf 'outra base\n' > a.txt; else printf 'um\n' > a.txt; fi
    printf 'ignorado.log\n.design-sync-reports/\n' > .gitignore
    g add -A
    GIT_AUTHOR_DATE=2026-11-01T10:00:00Z GIT_COMMITTER_DATE=2026-11-01T10:00:00Z g commit -q -m base
    [ "$var" = branch-existe ] && g branch feat/y
    ;;
esac

cd "$HOME"
case $var in
  plano-diferente) mkdir -p plans; printf '# Plano diferente\n' > plans/demo.md ;;
  worktree-existe) mkdir -p trees/fake-a; printf 'x\n' > trees/fake-a/x.txt ;;
  registro-existe)
    mkdir -p work/_skill/orchestration/agents
    printf '{"name":"fake-a","state":"working"}\n' > work/_skill/orchestration/agents/fake-a.json ;;
  truncado) head -c 600 "$pacote" > "$pacote.t"; mv "$pacote.t" "$pacote" ;;
  nao-pacote) printf 'isto nao e um pacote\n' > "$pacote" ;;
  log2n) sed -i 's/"log2n":14/"log2n":99/' "$pacote" ;;
  header) sed -i 's/"kdf":"scrypt"/"kdf":"scryp7"/' "$pacote" ;;
esac
:
