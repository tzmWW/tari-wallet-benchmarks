#!/usr/bin/env bash
set -euo pipefail

# Regenerates the machine-managed development stack snapshot block in README.md
# from a resolved tools/build-manifest.json and the committed Cargo.lock.
# Exits 0 without writing when nothing changed so CI can skip creating pull
# requests. Prose outside the markers is never modified.

MANIFEST="${1:-tools/build-manifest.json}"
README="${2:-README.md}"
CARGO_LOCK="${3:-Cargo.lock}"
TARI_CHECKOUT="${4:-.bench-cache/dev/tari}"
START_MARKER="<!-- dev-stack-snapshot:start -->"
END_MARKER="<!-- dev-stack-snapshot:end -->"

for file in "$MANIFEST" "$README" "$CARGO_LOCK"; do
  if [ ! -f "$file" ]; then
    printf '%s not found\n' "$file" >&2
    exit 1
  fi
done
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required" >&2
  exit 1
fi

resolved_date="$(jq -r '.resolved_at' "$MANIFEST" | cut -d T -f 1)"
tari_commit="$(jq -r '.sources.tari_console_wallet.upstream.commit' "$MANIFEST")"
pp_commit="$(jq -r '.sources.payment_processor.upstream.commit' "$MANIFEST")"
minotari_commit="$(jq -r '.sources.minotari_cli.upstream.commit' "$MANIFEST")"

minotari_full="$(awk -v RS= '
  /name = "minotari"/ && /git\+https:\/\/github\.com\/tari-project\/minotari-cli\?branch=main#/ {
    if (match($0, /#[0-9a-f]{40}/)) {
      print substr($0, RSTART + 1, 40)
      exit
    }
  }' "$CARGO_LOCK")"
tari_version="$(awk -v RS= '
  /name = "tari_common"/ {
    if (match($0, /version = "[^"]+"/)) {
      print substr($0, RSTART + 11, RLENGTH - 12)
      exit
    }
  }' "$CARGO_LOCK")"

for value in "$resolved_date" "$tari_commit" "$pp_commit" "$minotari_full" "$tari_version"; do
  if [ -z "$value" ]; then
    echo "could not derive snapshot values from $MANIFEST and $CARGO_LOCK" >&2
    exit 1
  fi
done

if [ "$(grep -cF "$START_MARKER" "$README" || true)" != 1 ] ||
  [ "$(grep -cF "$END_MARKER" "$README" || true)" != 1 ]; then
  echo "snapshot markers must appear exactly once in $README" >&2
  exit 1
fi

tari_tag="$(git -C "$TARI_CHECKOUT" tag --points-at "$tari_commit" 2>/dev/null |
  grep '^v' | head -n 1 || true)"
if [ -z "$tari_tag" ]; then
  tari_tag="$(jq -r '.sources.tari_console_wallet.upstream.revision' "$MANIFEST")"
fi

block_file="$(mktemp)"
spliced_file="$(mktemp)"
trap 'rm -f "$block_file" "$spliced_file"' EXIT

cat > "$block_file" <<EOF
At this revision, \`Cargo.lock\` resolves Minotari \`main\` to \`${minotari_commit:0:8}\` and the Tari
API/runtime line is \`v${tari_version}\`. The last verified dev build on ${resolved_date}
selected Tari prerelease \`${tari_tag}\` at \`${tari_commit:0:8}\` and resolved
payment-processor \`main\` to \`${pp_commit:0:8}\`. These are not permanent allowlist
pins; the dev fetcher resolves the moving refs again and freezes their full
commits in each run manifest.
EOF

awk -v start="$START_MARKER" -v end="$END_MARKER" -v block="$block_file" '
  $0 == start {
    print
    while ((getline line < block) > 0) print line
    close(block)
    inside = 1
    next
  }
  $0 == end {
    print
    inside = 0
    next
  }
  !inside { print }
' "$README" > "$spliced_file"

if cmp -s "$README" "$spliced_file"; then
  echo "development stack snapshot already current"
  exit 0
fi

mv "$spliced_file" "$README"
echo "updated development stack snapshot in $README"
