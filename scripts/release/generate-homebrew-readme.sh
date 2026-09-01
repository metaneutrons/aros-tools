#!/usr/bin/env bash

# Trusted local renderer for the metaneutrons tap README.  Publication must
# never execute a helper fetched from the writable destination repository while
# the tap PAT is in scope.
set -euo pipefail

tap_root=${1:-}
[[ -d "$tap_root/Formula" && ! -L "$tap_root" && \
   ! -L "$tap_root/Formula" && ! -L "$tap_root/README.md" ]] || {
    printf '%s\n' '::error::AP7300 tap root is missing or unsafe' >&2
    exit 1
}

extract_string() {
    local key=$1 formula=$2
    sed -nE "s/^[[:space:]]*${key} \"([^\"]+)\".*/\\1/p" "$formula" | head -n 1
}

resolve_version() {
    local formula=$1 version
    version=$(extract_string version "$formula")
    if [[ -z "$version" ]]; then
        version=$(sed -nE 's/^[[:space:]]*url "([^"]+)".*/\1/p' "$formula" | \
            grep -Eo '[0-9]+(\.[0-9]+)+' | head -n 1 || true)
    fi
    [[ -n "$version" ]] || {
        printf '::error::AP7300 cannot derive version from %s\n' "$formula" >&2
        exit 1
    }
    printf '%s' "$version"
}

output="$tap_root/README.md"
temporary=$(mktemp "$tap_root/.README.md.XXXXXX")
cleanup() { unlink "$temporary" 2>/dev/null || true; }
trap cleanup EXIT
cat > "$temporary" <<'EOF'
# 🍺 Homebrew Tap - metaneutrons

Custom Homebrew formulas for metaneutrons projects.

## 📦 Installation

First, tap this repository:

```bash
brew tap metaneutrons/tap
```

## 🚀 Available Formulas
EOF

found=false
while IFS= read -r formula; do
    found=true
    name=$(basename "$formula" .rb)
    description=$(extract_string desc "$formula")
    homepage=$(extract_string homepage "$formula")
    version=$(resolve_version "$formula")
    [[ -n "$description" && -n "$homepage" ]] || {
        printf '::error::AP7300 incomplete formula metadata in %s\n' "$formula" >&2
        exit 1
    }
    cat >> "$temporary" <<EOF

### $name

**Description:** $description

**Version:** $version

**Homepage:** $homepage

**Installation:**

\`\`\`bash
brew install metaneutrons/tap/$name
\`\`\`
EOF
done < <(find "$tap_root/Formula" -maxdepth 1 -type f -name '*.rb' -print | LC_ALL=C sort)
[[ "$found" == true ]] || {
    printf '%s\n' '::error::AP7300 tap has no formulas' >&2
    exit 1
}

cat >> "$temporary" <<'EOF'

## 💻 Usage

After installation, you can use any of the tools directly from your terminal.

## 🐛 Issues

If you encounter any issues with these formulas, please report them in the respective project repositories.

## 🤝 Contributing

Formula changes and the generated README must be submitted together in one pull request. Run `scripts/generate-readme.sh` after changing a formula.
EOF
chmod 0644 "$temporary"
mv "$temporary" "$output"
trap - EXIT
