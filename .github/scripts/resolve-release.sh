#!/usr/bin/env bash
# Decides what release.yml ships. Prints `key=value` lines for $GITHUB_OUTPUT.
#
# Channels:
#   stable   A `vX.Y.Z` tag push, or a manual run with channel=stable. Built
#            from main. The version is Cargo.toml's; a tag that disagrees
#            fails, so the version is bumped in a reviewed commit first.
#   nightly  The daily schedule, or a manual run with channel=nightly. Built
#            from develop. Version `<next patch>-nightly.<yyyymmdd>.<run>`.
#            Skipped when HEAD is already the latest nightly.
#
# Env: EVENT_NAME, DISPATCH_CHANNEL, REF_NAME, RUN_NUMBER.
# Needs a full clone with tags (fetch-depth: 0).
set -euo pipefail

fail() {
  echo "::error::$*" >&2
  exit 1
}

out() {
  echo "$1=$2"
}

cargo_version="$(sed -nE '/^\[workspace\.package\]/,/^\[/ s/^version = "([^"]+)"$/\1/p' Cargo.toml)"
[[ -n "$cargo_version" ]] || fail "No [workspace.package] version in Cargo.toml."

semver='^([0-9]+)\.([0-9]+)\.([0-9]+)(-[0-9A-Za-z.-]+)?$'
[[ "$cargo_version" =~ $semver ]] || fail "Cargo.toml version $cargo_version is not semver."
major="${BASH_REMATCH[1]}" minor="${BASH_REMATCH[2]}" patch="${BASH_REMATCH[3]}"
prerelease="${BASH_REMATCH[4]:-}"

sha="$(git rev-parse HEAD)"

case "$EVENT_NAME" in
  schedule) channel=nightly ;;
  workflow_dispatch) channel="${DISPATCH_CHANNEL:-nightly}" ;;
  push) channel=stable ;;
  *) fail "Unsupported event: $EVENT_NAME" ;;
esac

# Stable ships main, nightly ships develop (AGENTS.md#git-workflow). A tag on
# a work branch would publish code that never passed review.
branch="$([[ "$channel" == stable ]] && echo main || echo develop)"
git fetch --quiet --no-tags origin "$branch"
git merge-base --is-ancestor "$sha" "origin/$branch" \
  || fail "$sha is not on $branch. $channel releases are cut from $branch only."

if [[ "$channel" == stable ]]; then
  version="$cargo_version"
  tag="v$version"
  if [[ "$EVENT_NAME" == push ]]; then
    [[ "$REF_NAME" == "$tag" ]] \
      || fail "Tag $REF_NAME does not match Cargo.toml version $cargo_version. Bump Cargo.toml first."
  elif git rev-parse --quiet --verify "refs/tags/$tag" > /dev/null; then
    fail "$tag already exists. Bump the version in Cargo.toml to release again."
  fi

  previous_tag="$(git tag --list 'v*' --sort=-v:refname | grep -v -- '-nightly\.' | grep -vxF "$tag" | head -n 1 || true)"
  out should_release true
  out is_prerelease "$([[ -n "$prerelease" ]] && echo true || echo false)"
  out make_latest "$([[ -n "$prerelease" ]] && echo false || echo true)"
elif [[ "$channel" == nightly ]]; then
  latest_nightly="$(git tag --list 'v*-nightly.*' --sort=-creatordate | head -n 1 || true)"
  if [[ "$EVENT_NAME" == schedule && -n "$latest_nightly" \
    && "$(git rev-parse "$latest_nightly^{commit}")" == "$sha" ]]; then
    echo "HEAD is already $latest_nightly. Nothing to release." >&2
    out should_release false
    exit 0
  fi

  # A nightly previews the next release. Once Cargo.toml's version has shipped
  # as stable, nightlies move to the next patch so they sort above it.
  base="$major.$minor.$patch"
  if git rev-parse --quiet --verify "refs/tags/v$base" > /dev/null; then
    base="$major.$minor.$((patch + 1))"
  fi
  date="$(date -u +%Y%m%d)"
  version="$base-nightly.$date.$RUN_NUMBER"
  tag="v$version"
  previous_tag="$latest_nightly"
  out should_release true
  out is_prerelease true
  out make_latest false
else
  fail "Unknown channel: $channel"
fi

out channel "$channel"
out version "$version"
out tag "$tag"
out name "Aeon $tag"
out previous_tag "$previous_tag"
out ref "$sha"
