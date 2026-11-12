# Version and build number of a package, for PKGBUILDs (Labyrinth_OS docs/VERSIONING.md). Every recipe in packages/pkgbuilds
# sources this file. The lines between the two markers are the same in Labyrinth_OS (scripts/package-version.sh there, and
# the copies in its store recipes); its tests/test_package_version.py checks that they stay the same.
# --- package version (scripts/package-version.sh) ---
# _version CHECKOUT [PATH...]: sets pkgver and pkgrel. pkgver is the VERSION file at the top of CHECKOUT's git repository (as
# committed); pkgrel, the build number, is 1 + the commits since that file last changed (the release) that touch the PATHs
# in that repository (relative ones are from its top), with the Rust crates they use from it (their path dependencies), or
# the whole repository when no PATH is in it. The recipe's own folder counts too; so does every PATH in another repository
# (by date: its commits since the release). Without a VERSION file (no clone yet: makepkg reads a recipe before it fetches)
# pkgver and pkgrel stay as they are.
_version() {
  local top base since rel=1 a q t todo=() paths=() others=("${startdir:-}")
  top=$(git -C "$1" rev-parse --show-toplevel 2>/dev/null) && base=$(git -C "$top" log -1 --format=%H -- VERSION 2>/dev/null) \
    && [[ -n $base ]] || return 0
  for a in "${@:2}"; do
    [[ $a == /* ]] || a=$top/$a
    a=$(realpath -m "$a")
    if [[ $a == "$top" || $a == "$top"/* ]]; then todo+=("$a"); else others+=("$a"); fi
  done
  while ((${#todo[@]})); do
    a=${todo[0]}; todo=("${todo[@]:1}")
    [[ " ${paths[*]} " == *" $a "* ]] && continue
    paths+=("$a")
    for q in $(sed -n 's/^[^#]*path *= *"\(\.\.[^"]*\)".*/\1/p' "$a/Cargo.toml" 2>/dev/null); do todo+=("$(realpath -m "$a/$q")"); done
  done
  since=$(git -C "$top" log -1 --format=%ct "$base")
  for a in "${others[@]}"; do
    [[ -n $a ]] && t=$(git -C "$a" rev-parse --show-toplevel 2>/dev/null) || continue
    if [[ $t == "$top" ]]; then ((${#paths[@]})) && paths+=("$(realpath "$a")")
    # another checkout of the same repository (an app's own recipe, built from a clone of it): counted above already
    elif [[ $(git -C "$t" rev-list --max-parents=0 HEAD) == "$(git -C "$top" rev-list --max-parents=0 HEAD)" ]]; then continue
    else rel=$((rel + $(git -C "$a" rev-list --count --since="@$since" HEAD -- .))); fi
  done
  pkgver=$(git -C "$top" show HEAD:VERSION)
  pkgrel=$((rel + $(git -C "$top" rev-list --count "$base..HEAD" -- "${paths[@]}")))
}
# _srcdir: makepkg's $srcdir, also while makepkg reads the recipe before setting it (for the clones of -git recipes)
_srcdir() {
  if [[ -n ${srcdir:-} ]]; then echo "$srcdir"
  elif [[ ${BUILDDIR:-} -ef ${startdir:-} ]]; then echo "$BUILDDIR/src"
  else echo "${BUILDDIR:-}/$pkgname/src"; fi
}
# --- end of package version ---
