#!/usr/bin/env bash
# Sample Bash file to exercise syntax highlighting, indents,
# and folds in vorto. Open with `vorto assets/samples/hello.sh`.

set -euo pipefail

readonly GREETING="${GREETING:-Hello}"
declare -a PEOPLE=("Alice" "Bob" "Carol")

greet() {
    local name="$1"
    local prefix="${2:-$GREETING}"
    printf '%s, %s!\n' "$prefix" "$name"
}

classify() {
    local n="$1"
    if ((n < 0)); then
        echo "negative"
    elif ((n == 0)); then
        echo "zero"
    elif ((n % 2 == 0)); then
        echo "positive even"
    else
        echo "positive odd"
    fi
}

main() {
    for name in "${PEOPLE[@]}"; do
        greet "$name"
    done

    for n in $(seq -2 5); do
        printf '%d -> %s\n' "$n" "$(classify "$n")"
    done

    case "$(uname -s)" in
        Darwin) echo "running on macOS" ;;
        Linux) echo "running on Linux" ;;
        *) echo "unknown platform" ;;
    esac
}

main "$@"
