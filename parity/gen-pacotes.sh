#!/bin/sh
# Gera os pacotes fixos de parity/fixtures/unpack/pacotes: ts.caronte (TS no HADES_SKILL_SHA) e rust.caronte (binário),
# da fixture orq com setup-orq.sh, senha, log2n e HADES_NOW fixos. Regenerar é PR explícita, como subir o sha.
# Uso: parity/gen-pacotes.sh <checkout do Hades-Skill> <binário caronte> [bun]
set -eu
skill=${1:?checkout do Hades-Skill}; rust=${2:?binario caronte}; bun=${3:-bun}
here=$(cd "$(dirname "$0")" && pwd)
dest=$here/fixtures/unpack/pacotes
mkdir -p "$dest"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

for side in ts rust; do
  home=$tmp/$side/home; mkdir -p "$home"
  cp -R "$here/fixtures/orq/." "$home/"
  grep -rIl '@HOME@' "$home" | while read -r f; do sed -i "s|@HOME@|$home|g" "$f"; done
  (cd "$home" && HOME=$home SIDE=rust ORQ_IGNORE_REPORTS=1 sh "$here/setup-orq.sh")
  export HOME=$home HADES_SKILL_DIR=$skill HADES_WORK_ROOT=$home/work HADES_NOW=2026-11-20T12:00:00Z \
    HADES_CARONTE_SCRYPT_LOG2N=14 HADES_CARONTE_PASSWORD="senha de teste 123"
  cd "$home"
  if [ "$side" = ts ]; then "$bun" "$skill/src/cli.ts" caronte pack demo --out out/p.caronte
  else "$rust" pack demo --out out/p.caronte; fi
  cp out/p.caronte "$dest/$side.caronte"
done
