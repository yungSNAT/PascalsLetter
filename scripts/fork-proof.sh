#!/usr/bin/env bash
#
# Prove each row of README's 'What PascalsLetter changes' table against upstream
# RTK: its tests pass here and fail on upstream. A test that passes upstream
# means upstream has fixed the bug, so the patch and its row can go.
#
# Adapted from kylehgc/rtk scripts/fork-proof.sh at commit
# 6fb87aa9591f03815fe8932b7f241820e2bc9374 (Apache-2.0). Changed here: the
# manifest maps each README row to its proving tests and their platform; a row
# without a manifest line, or a line without a row, fails; only the manifest's
# Linux test files, with tests/common and tests/fixtures, are lifted onto
# upstream, including a test of the proxy/ crate; rows for another platform are
# marked, never proved; skip guards panic under RTK_PROOF; README's Upstream
# column must match the result; the table is printed, not written to a page.
#
# Linux only. Usage: scripts/fork-proof.sh
set -euo pipefail

upstream_ref=${UPSTREAM_REF:-upstream/develop}
claims=.github/fork-claims.tsv
readme=README.md

die() {
  echo "error: $*" >&2
  exit 1
}

[ $# -eq 0 ] || die "usage: $0"
git rev-parse --verify --quiet "$upstream_ref^{commit}" >/dev/null ||
  die "$upstream_ref not found: fetch rtk-ai/rtk develop into it"
[ -f "$claims" ] || die "$claims not found"

# A skipped test prints ok; under RTK_PROOF the proving tests' skip guards panic instead.
export RTK_PROOF=1

problems=()
declare -A cell claimed status

fail_on_problems() {
  [ ${#problems[@]} -gt 0 ] || return 0
  echo >&2
  printf 'fork-proof: %s\n' "${problems[@]}" >&2
  exit 1
}

# README rows as `area<TAB>Upstream cell`, in table order.
mapfile -t rows < <(awk -F' [|] ' '
  /^## / { s = ($0 == "## What PascalsLetter changes") }
  s && /^[|] / && !/^[|] (Area|---) / { sub(/^[|] /, ""); print $1 "\t" $2 }' "$readme")
[ ${#rows[@]} -gt 0 ] || die "no table under 'What PascalsLetter changes' in $readme"
for row in "${rows[@]}"; do cell["${row%%$'\t'*}"]=${row#*$'\t'}; done

lines=()
while IFS=$'\t' read -r area platform target test _; do
  case $area in '' | '#'*) continue ;; esac
  [ -n "${cell[$area]+set}" ] || problems+=("$area: in $claims, but not a README row")
  claimed[$area]=1
  lines+=("$area"$'\t'"$platform"$'\t'"$target"$'\t'"$test")
done <"$claims"
for row in "${rows[@]}"; do
  area=${row%%$'\t'*}
  [ -n "${claimed[$area]+set}" ] || problems+=("$area: claim without a test (no line in $claims)")
done
# A manifest out of step with README fails before anything is built.
fail_on_problems

targets=$(for line in "${lines[@]}"; do
  IFS=$'\t' read -r _ platform target _ <<<"$line"
  [ "$platform" != linux ] || echo "$target"
done | sort -u)

# `<target> <test> ok|FAILED` for each test cargo reports, run in $1 with any
# further arguments passed to cargo. Cargo's own output goes to stderr, so a
# compile error stays visible.
run_tests() {
  local target crate
  for target in $targets; do
    crate=.
    case $target in */tests/*) crate=${target%%/tests/*} ;; esac
    (cd "$1" && cargo test "${@:2}" --manifest-path "$crate/Cargo.toml" \
      --test "$(basename "$target" .rs)" 2>&1 || true) |
      awk -v t="$target" '{ print > "/dev/stderr" }
        /^test [^ ]+ \.\.\. (ok|FAILED)$/ { print t, $2, $4 }'
  done
}

echo "==> fork tests" >&2
here=$(run_tests . --locked)

# Under target/, which git ignores. Upstream builds in a target directory of its
# own beside the worktree, never over the fork's binaries, and kept for the next run.
upstream=target/fork-proof/upstream
remove_upstream() {
  git worktree remove --force "$upstream" 2>/dev/null || rm -rf "$upstream"
  git worktree prune
}
remove_upstream
trap remove_upstream EXIT
git worktree add --quiet --detach "$upstream" "$upstream_ref"
for path in $targets tests/common tests/fixtures; do
  mkdir -p "$upstream/$(dirname "$path")"
  cp -R "$path" "$upstream/$(dirname "$path")/"
done
echo "==> the same tests on $upstream_ref" >&2
# Not --locked: a lockfile upstream left behind its manifest (proxy/Cargo.lock
# on 12 November 2026) would stop the proof compiling there, which reads as a
# missing test rather than a result.
there=$(CARGO_TARGET_DIR=$PWD/target/fork-proof/target run_tests "$upstream")

result() {
  local r
  r=$(awk -v key="$2 $3" 'index($0, key " ") == 1 { print $3; exit }' <<<"$1")
  echo "${r:-MISSING}"
}

table=()
for line in "${lines[@]}"; do
  IFS=$'\t' read -r area platform target test <<<"$line"
  case $platform in
    linux) ;;
    windows) mark='Not proved: Windows only' ;;
    macos) mark='Not proved: macOS only' ;;
    none) mark='Not proved: no test' ;;
    *)
      problems+=("$area: unknown platform '$platform'")
      continue
      ;;
  esac
  if [ "$platform" != linux ]; then
    : "${status[$area]:=$mark}"
    table+=("$area"$'\t'"$test"$'\tnot run\tnot run')
    continue
  fi
  fork=$(result "$here" "$target" "$test")
  up=$(result "$there" "$target" "$test")
  table+=("$area"$'\t'"$test"$'\t'"$fork"$'\t'"$up")
  if [ "$fork" != ok ]; then
    problems+=("$area: $test does not pass on the fork ($fork)")
  elif [ "$up" = ok ]; then
    problems+=("$area: $test passes upstream: upstream has fixed this: drop the patch and the row")
  elif [ "$up" = MISSING ]; then
    problems+=("$area: $test did not compile or run upstream")
  else
    status[$area]='Not in upstream'
  fi
done

for row in "${rows[@]}"; do
  area=${row%%$'\t'*}
  [ -z "${status[$area]+set}" ] || [ "${cell[$area]}" = "${status[$area]}" ] ||
    problems+=("$area: README says '${cell[$area]}', the proof says '${status[$area]}'")
done

echo "Proof against \`$upstream_ref\` ($(git rev-parse --short "$upstream_ref")):"
echo
echo '| Area | Status | Test | Fork | Upstream |'
echo '| --- | --- | --- | --- | --- |'
for entry in "${table[@]}"; do
  IFS=$'\t' read -r area test fork up <<<"$entry"
  [ "$test" = - ] || test="\`$test\`"
  echo "| $area | ${status[$area]:-not proved} | $test | $fork | $up |"
done

fail_on_problems
