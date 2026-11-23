#!/usr/bin/env bash
# Maps changed paths to the CI gates they need. Reads one path per line from
# the file given as $1 and prints `gate=true|false` lines for $GITHUB_OUTPUT.
#
# A gate is `true` when any changed path can affect it. Unknown paths do not
# open a gate: docs and READMEs never need a Windows runner. ci.yml and this
# detector open every gate, so a change to the gating itself is always proven.
#
# Usage: ci-changes.sh <paths-file>   # decide from a diff
#        ci-changes.sh --all          # open every gate (main pushes, fail-open)
set -euo pipefail

gates=(rust packaging)

emit_all() {
  for gate in "${gates[@]}"; do echo "$gate=true"; done
}

if [[ "${1:-}" == "--all" ]]; then
  emit_all
  exit 0
fi

rust=false
packaging=false

while IFS= read -r path; do
  case "$path" in
    .github/workflows/ci.yml | .github/actions/ci-changes/* | .github/scripts/ci-changes.sh)
      emit_all
      exit 0
      ;;
    # Toolchain and dependency changes are what break the arm64 cross build,
    # so they also rerun packaging.
    Cargo.toml | Cargo.lock | rust-toolchain.toml | .cargo/*)
      rust=true
      packaging=true
      ;;
    crates/* | .github/workflows/rust-checks.yml | .github/actions/setup-rust/*)
      rust=true
      ;;
    packaging/* | scripts/package-msi.ps1 | .github/workflows/release*.yml | .github/actions/setup-wix/* | .github/scripts/*)
      packaging=true
      ;;
  esac
done < "$1"

echo "rust=$rust"
echo "packaging=$packaging"
