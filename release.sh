#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO_TOML="${ROOT_DIR}/Cargo.toml"
CARGO_LOCK="${ROOT_DIR}/Cargo.lock"
DRY_RUN=0
BACKFILL_VERSION=''
LINUX_WORKFLOW_FILE='linux-packages.yml'
WORKFLOW_START_POLL_INTERVAL_SEC=5
WORKFLOW_START_POLL_ATTEMPTS=60

# shellcheck source=scripts/load-dotenv.sh
if [[ -f "${ROOT_DIR}/scripts/load-dotenv.sh" ]]; then
    . "${ROOT_DIR}/scripts/load-dotenv.sh"
fi

breaking_notes=()
feature_notes=()
fix_notes=()
perf_notes=()
refactor_notes=()
docs_notes=()
build_notes=()
ci_notes=()
test_notes=()
chore_notes=()
revert_notes=()
other_notes=()
release_assets=()
local_macos_pkg_candidates=()
conventional_commit_regex='^([[:alnum:]-]+)(\(([^)]*)\))?(!)?:[[:space:]](.+)$'

usage() {
    cat <<'EOF'
Create interactive NeoMist release.

Usage:
  ./release.sh [--dry-run]
  ./release.sh --backfill VERSION [--dry-run]

Options:
  --backfill VERSION  Create GitHub release for existing tag/workflow artifacts without new commit or tag push
  --dry-run           Show proposed version, notes, commit, tag, and push targets without changing git state
  -h, --help          Show help

Flow:
  1. Read commits since latest v* tag
  2. Build release notes from semantic commit subjects
  3. Suggest next version and ask for confirmation
  4. Update Cargo.toml and Cargo.lock when needed
  5. Commit release version when needed, create annotated tag, and push branch + tag to upstream remote
  6. Optionally build/sign/notarize macOS pkg on macOS host
  7. Wait for Linux Packages workflow, download release artifacts, and create GitHub release with all assets

Backfill flow:
  1. Read commits for existing tag range
  2. Show release notes for existing version
  3. Download existing Linux Packages workflow artifacts from GitHub
  4. Optionally attach local macOS pkg(s) from dist/
  5. Create GitHub release for existing tag
EOF
}

die() {
    printf '%s\n' "$1" >&2
    exit 1
}

require_command() {
    local command_name=$1
    command -v "$command_name" >/dev/null 2>&1 || die "Missing required command: ${command_name}"
}

ensure_gh_ready() {
    require_command gh
    gh auth status >/dev/null 2>&1 || die 'GitHub CLI not authenticated. Run gh auth login.'
}

is_github_remote_url() {
    local url=$1
    [[ "$url" == *'github.com:'* || "$url" == *'github.com/'* ]]
}

resolve_github_remote() {
    local preferred_remote=${1:-}
    local remote url

    if [[ -n "$preferred_remote" ]]; then
        url="$(git remote get-url "$preferred_remote" 2>/dev/null || true)"
        if is_github_remote_url "$url"; then
            printf '%s\n' "$preferred_remote"
            return 0
        fi
    fi

    if git remote get-url github >/dev/null 2>&1; then
        url="$(git remote get-url github)"
        if is_github_remote_url "$url"; then
            printf 'github\n'
            return 0
        fi
    fi

    while IFS= read -r remote; do
        url="$(git remote get-url "$remote" 2>/dev/null || true)"
        if is_github_remote_url "$url"; then
            printf '%s\n' "$remote"
            return 0
        fi
    done < <(git remote)

    return 1
}

validate_semver() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

semver_compare() {
    local left=$1
    local right=$2
    local left_major left_minor left_patch right_major right_minor right_patch

    validate_semver "$left" || die "Invalid semantic version: ${left}"
    validate_semver "$right" || die "Invalid semantic version: ${right}"

    IFS=. read -r left_major left_minor left_patch <<<"$left"
    IFS=. read -r right_major right_minor right_patch <<<"$right"

    if (( left_major > right_major )); then
        printf '1\n'
        return 0
    fi
    if (( left_major < right_major )); then
        printf '%s\n' '-1'
        return 0
    fi
    if (( left_minor > right_minor )); then
        printf '1\n'
        return 0
    fi
    if (( left_minor < right_minor )); then
        printf '%s\n' '-1'
        return 0
    fi
    if (( left_patch > right_patch )); then
        printf '1\n'
        return 0
    fi
    if (( left_patch < right_patch )); then
        printf '%s\n' '-1'
        return 0
    fi

    printf '0\n'
}

semver_bump() {
    local version=$1
    local level=$2
    local major minor patch

    validate_semver "$version" || die "Invalid semantic version: ${version}"
    IFS=. read -r major minor patch <<<"$version"

    case "$level" in
        major)
            printf '%s\n' "$((major + 1)).0.0"
            ;;
        minor)
            printf '%s\n' "${major}.$((minor + 1)).0"
            ;;
        patch)
            printf '%s\n' "${major}.${minor}.$((patch + 1))"
            ;;
        *)
            die "Unknown bump level: ${level}"
            ;;
    esac
}

max_semver() {
    local left=$1
    local right=$2
    local comparison

    comparison="$(semver_compare "$left" "$right")"
    if [[ "$comparison" == "-1" ]]; then
        printf '%s\n' "$right"
    else
        printf '%s\n' "$left"
    fi
}

read_cargo_version() {
    awk -F '"' '
        /^\[package\]$/ { in_package = 1; next }
        /^\[/ && $0 != "[package]" { in_package = 0 }
        in_package && /^version = "/ { print $2; exit }
    ' "$CARGO_TOML"
}

ensure_clean_worktree() {
    local status_output

    status_output="$(git status --porcelain --untracked-files=all)"
    if [[ -n "$status_output" ]]; then
        die "Working tree not clean. Commit, stash, or remove pending changes before release."
    fi
}

prompt_yes_no() {
    local prompt=$1
    local default=$2
    local response normalized

    while true; do
        if [[ "$default" == "y" ]]; then
            read -r -p "${prompt} [Y/n] " response || exit 1
            response="${response:-Y}"
        else
            read -r -p "${prompt} [y/N] " response || exit 1
            response="${response:-N}"
        fi

        normalized="$(printf '%s' "$response" | tr '[:upper:]' '[:lower:]')"
        case "$normalized" in
            y|yes)
                return 0
                ;;
            n|no)
                return 1
                ;;
        esac

        printf 'Enter y or n.\n' >&2
    done
}

prompt_version() {
    local suggested_version=$1
    local latest_tag_version=${2:-}
    local current_version=$3
    local candidate comparison

    while true; do
        read -r -p "Release version [${suggested_version}]: " candidate || exit 1
        candidate="${candidate:-$suggested_version}"

        if ! validate_semver "$candidate"; then
            printf 'Version must match MAJOR.MINOR.PATCH.\n' >&2
            continue
        fi

        if [[ -n "$latest_tag_version" ]]; then
            comparison="$(semver_compare "$candidate" "$latest_tag_version")"
            if [[ "$comparison" != "1" ]]; then
                printf 'Version must be greater than latest tag version %s.\n' "$latest_tag_version" >&2
                continue
            fi
        fi

        comparison="$(semver_compare "$candidate" "$current_version")"
        if [[ "$comparison" == "-1" ]]; then
            printf 'Version must not be lower than current Cargo version %s.\n' "$current_version" >&2
            continue
        fi

        printf '%s\n' "$candidate"
        return 0
    done
}

note_line() {
    local description=$1
    local scope=$2
    local short_sha=$3

    if [[ -n "$scope" ]]; then
        printf -- '- %s: %s (`%s`)' "$scope" "$description" "$short_sha"
    else
        printf -- '- %s (`%s`)' "$description" "$short_sha"
    fi
}

reset_release_notes() {
    breaking_notes=()
    feature_notes=()
    fix_notes=()
    perf_notes=()
    refactor_notes=()
    docs_notes=()
    build_notes=()
    ci_notes=()
    test_notes=()
    chore_notes=()
    revert_notes=()
    other_notes=()
}

populate_release_notes_from_commit_range() {
    local commit_range=$1
    local release_level='patch'
    local local_short_sha raw_type scope bang description type line has_breaking_body

    reset_release_notes

    while IFS= read -r -d '' sha && IFS= read -r -d '' subject && IFS= read -r -d '' body; do
        local_short_sha="${sha:0:7}"
        if [[ "$subject" =~ $conventional_commit_regex ]]; then
            raw_type="${BASH_REMATCH[1]}"
            scope="${BASH_REMATCH[3]}"
            bang="${BASH_REMATCH[4]}"
            description="${BASH_REMATCH[5]}"
            type="$(printf '%s' "$raw_type" | tr '[:upper:]' '[:lower:]')"
            line="$(note_line "$description" "$scope" "$local_short_sha")"

            case "$body" in
                *"BREAKING CHANGE:"*|*"BREAKING-CHANGE:"*)
                    has_breaking_body=1
                    ;;
                *)
                    has_breaking_body=0
                    ;;
            esac

            if [[ -n "$bang" || "$has_breaking_body" == '1' ]]; then
                breaking_notes+=("$line")
                release_level='major'
            fi

            case "$type" in
                feat)
                    feature_notes+=("$line")
                    if [[ "$release_level" != 'major' ]]; then
                        release_level='minor'
                    fi
                    ;;
                fix)
                    fix_notes+=("$line")
                    ;;
                perf)
                    perf_notes+=("$line")
                    ;;
                refactor)
                    refactor_notes+=("$line")
                    ;;
                docs)
                    docs_notes+=("$line")
                    ;;
                build)
                    build_notes+=("$line")
                    ;;
                ci)
                    ci_notes+=("$line")
                    ;;
                test)
                    test_notes+=("$line")
                    ;;
                chore)
                    chore_notes+=("$line")
                    ;;
                revert)
                    revert_notes+=("$line")
                    ;;
                *)
                    other_notes+=("- ${subject} (\`${local_short_sha}\`)")
                    ;;
            esac
        else
            other_notes+=("- ${subject} (\`${local_short_sha}\`)")
        fi
    done < <(git log "$commit_range" --no-merges --reverse --format='%H%x00%s%x00%b%x00')

    printf '%s\n' "$release_level"
}

render_section() {
    local output_path=$1
    local title=$2
    local array_name=$3
    local count item

    eval "count=\${#${array_name}[@]}"
    if [[ "$count" -eq 0 ]]; then
        return 0
    fi

    printf '## %s\n\n' "$title" >> "$output_path"
    eval "for item in \"\${${array_name}[@]}\"; do printf '%s\\n' \"\$item\" >> \"$output_path\"; done"
    printf '\n' >> "$output_path"
}

write_release_notes() {
    local output_path=$1
    local release_version=$2
    local latest_tag=$3
    local commit_count=$4

    {
        printf 'Release v%s\n\n' "$release_version"
        if [[ -n "$latest_tag" ]]; then
            printf 'Changes since %s. %s commit(s).\n\n' "$latest_tag" "$commit_count"
        else
            printf 'Initial tagged release. %s commit(s).\n\n' "$commit_count"
        fi
    } > "$output_path"

    render_section "$output_path" 'Breaking Changes' breaking_notes
    render_section "$output_path" 'Features' feature_notes
    render_section "$output_path" 'Fixes' fix_notes
    render_section "$output_path" 'Performance' perf_notes
    render_section "$output_path" 'Refactors' refactor_notes
    render_section "$output_path" 'Docs' docs_notes
    render_section "$output_path" 'Build' build_notes
    render_section "$output_path" 'CI' ci_notes
    render_section "$output_path" 'Tests' test_notes
    render_section "$output_path" 'Chores' chore_notes
    render_section "$output_path" 'Reverts' revert_notes
    render_section "$output_path" 'Other' other_notes
}

update_cargo_toml_version() {
    local new_version=$1
    local temp_file

    temp_file="$(mktemp "${TMPDIR:-/tmp}/neomist-cargo-toml.XXXXXX")"
    awk -v version="$new_version" '
        BEGIN { in_package = 0; updated = 0 }
        /^\[package\]$/ { in_package = 1; print; next }
        /^\[/ && $0 != "[package]" { in_package = 0 }
        in_package && /^version = "/ && updated == 0 {
            sub(/"[^"]+"/, "\"" version "\"")
            updated = 1
        }
        { print }
        END { if (updated == 0) exit 1 }
    ' "$CARGO_TOML" > "$temp_file" || {
        rm -f "$temp_file"
        die 'Failed to update Cargo.toml version.'
    }

    mv "$temp_file" "$CARGO_TOML"
}

update_cargo_lock_version() {
    local new_version=$1
    local temp_file

    temp_file="$(mktemp "${TMPDIR:-/tmp}/neomist-cargo-lock.XXXXXX")"
    awk -v version="$new_version" '
        BEGIN { in_package = 0; is_target = 0; updated = 0 }
        /^\[\[package\]\]$/ {
            in_package = 1
            is_target = 0
            print
            next
        }
        in_package && /^name = "neomist"$/ {
            is_target = 1
            print
            next
        }
        in_package && is_target && /^version = "/ && updated == 0 {
            sub(/"[^"]+"/, "\"" version "\"")
            updated = 1
            is_target = 0
            print
            next
        }
        { print }
        END { if (updated == 0) exit 1 }
    ' "$CARGO_LOCK" > "$temp_file" || {
        rm -f "$temp_file"
        die 'Failed to update Cargo.lock version.'
    }

    mv "$temp_file" "$CARGO_LOCK"
}

resolve_previous_release_tag() {
    local ref=$1
    git describe --tags --abbrev=0 --match 'v[0-9]*' "${ref}^" 2>/dev/null || true
}

resolve_release_repo_slug() {
    gh repo view --json nameWithOwner --jq '.nameWithOwner'
}

wait_for_linux_packages_run() {
    local repo_slug=$1
    local head_sha=$2
    local started_at=$3
    local attempt=0
    local run_id=''

    while [[ -z "$run_id" ]]; do
        run_id="$({
            gh api "repos/${repo_slug}/actions/workflows/${LINUX_WORKFLOW_FILE}/runs?event=push&head_sha=${head_sha}&per_page=20" \
                --jq ".workflow_runs | map(select(.created_at >= \"${started_at}\")) | sort_by(.created_at) | reverse | .[0].id // empty"
        } || true)"

        if [[ -n "$run_id" ]]; then
            printf '%s\n' "$run_id"
            return 0
        fi

        attempt=$((attempt + 1))
        if (( attempt >= WORKFLOW_START_POLL_ATTEMPTS )); then
            die 'Timed out waiting for Linux Packages workflow to start on GitHub.'
        fi

        sleep "$WORKFLOW_START_POLL_INTERVAL_SEC"
    done
}

download_linux_release_artifacts() {
    local repo_slug=$1
    local run_id=$2
    local output_dir=$3

    gh run watch "$run_id" --repo "$repo_slug" --exit-status
    gh run download "$run_id" --repo "$repo_slug" --dir "$output_dir"
}

collect_linux_release_assets() {
    local artifact_dir=$1
    local path

    release_assets=()
    while IFS= read -r -d '' path; do
        release_assets+=("$path")
    done < <(find "$artifact_dir" -type f \( -name '*.deb' -o -name '*.AppImage' \) -print0)

    if [[ "${#release_assets[@]}" -eq 0 ]]; then
        die 'No Linux release artifacts downloaded from GitHub workflow.'
    fi
}

collect_local_macos_pkg_candidates() {
    local version=$1
    local path

    local_macos_pkg_candidates=()
    for path in "${ROOT_DIR}/dist"/neomist-"${version}"-macos-*.pkg; do
        [[ -f "$path" ]] || continue
        local_macos_pkg_candidates+=("$path")
    done
}

create_github_release() {
    local repo_slug=$1
    local release_tag=$2
    local notes_path=$3

    if gh release view "$release_tag" --repo "$repo_slug" >/dev/null 2>&1; then
        die "GitHub release already exists: ${release_tag}"
    fi

    printf 'Creating GitHub release %s on %s...\n' "$release_tag" "$repo_slug"
    gh release create "$release_tag" "${release_assets[@]}" --repo "$repo_slug" --title "$release_tag" --notes-file "$notes_path"
    printf 'GitHub release created: %s\n' "$release_tag"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --backfill)
            if [[ $# -lt 2 ]]; then
                die 'Missing value for --backfill'
            fi
            BACKFILL_VERSION="$2"
            shift
            ;;
        --dry-run)
            DRY_RUN=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
    shift
done

require_command git
require_command awk

[[ -f "$CARGO_TOML" ]] || die "Missing file: ${CARGO_TOML}"
[[ -f "$CARGO_LOCK" ]] || die "Missing file: ${CARGO_LOCK}"

cd "$ROOT_DIR"

if [[ -n "$BACKFILL_VERSION" ]]; then
    backfill_version="${BACKFILL_VERSION#v}"
    validate_semver "$backfill_version" || die "Backfill version is not semantic: ${BACKFILL_VERSION}"

    ensure_gh_ready
    github_remote="$(resolve_github_remote)" || die 'No GitHub remote found in this repository.'
    git fetch "$github_remote" --tags >/dev/null

    release_repo_slug="$(resolve_release_repo_slug)"
    release_version="$backfill_version"
    release_tag="v${release_version}"
    git rev-parse -q --verify "refs/tags/${release_tag}" >/dev/null 2>&1 || die "Missing local tag: ${release_tag}"

    previous_tag="$(resolve_previous_release_tag "$release_tag")"
    commit_range="$release_tag"
    if [[ -n "$previous_tag" ]]; then
        commit_range="${previous_tag}..${release_tag}"
    fi

    commit_count="$(git rev-list --count --no-merges "$commit_range")"
    release_level="$(populate_release_notes_from_commit_range "$commit_range")"
    release_commit_sha="$(git rev-list -n 1 "$release_tag")"

    notes_file="$(mktemp "${TMPDIR:-/tmp}/neomist-release-notes.XXXXXX")"
    release_artifact_dir=''
    cleanup() {
        rm -f "$notes_file"
        rm -rf "$release_artifact_dir"
    }
    trap cleanup EXIT

    write_release_notes "$notes_file" "$release_version" "$previous_tag" "$commit_count"

    printf 'Backfill release version: %s\n' "$release_version"
    printf 'Release tag: %s\n' "$release_tag"
    if [[ -n "$previous_tag" ]]; then
        printf 'Previous tag: %s\n' "$previous_tag"
    fi
    printf 'Detected bump in notes: %s\n\n' "$release_level"

    cat "$notes_file"
    printf '\n'

    collect_local_macos_pkg_candidates "$release_version"
    selected_macos_assets=()
    if [[ "${#local_macos_pkg_candidates[@]}" -gt 0 ]]; then
        printf 'Found local macOS pkg(s):\n'
        for candidate_path in "${local_macos_pkg_candidates[@]}"; do
            printf '  %s\n' "$candidate_path"
        done
        printf '\n'

        if prompt_yes_no 'Include local macOS pkg(s) in GitHub release?' 'y'; then
            selected_macos_assets=("${local_macos_pkg_candidates[@]}")
        fi
    else
        printf 'No local macOS pkg found in dist/ for %s.\n\n' "$release_version"
    fi

    if ! prompt_yes_no "Proceed with GitHub release backfill ${release_tag}?" 'n'; then
        printf 'Backfill cancelled.\n'
        exit 0
    fi

    if [[ "$DRY_RUN" == '1' ]]; then
        printf 'Dry run. No git changes or GitHub release created.\n'
        printf 'Would download existing GitHub workflow %s artifacts for %s and create GitHub release %s.\n' "$LINUX_WORKFLOW_FILE" "$release_tag" "$release_tag"
        if [[ "${#selected_macos_assets[@]}" -gt 0 ]]; then
            printf 'Would attach %s local macOS pkg artifact(s).\n' "${#selected_macos_assets[@]}"
        fi
        exit 0
    fi

    release_artifact_dir="$(mktemp -d "${TMPDIR:-/tmp}/neomist-release-assets.XXXXXX")"
    printf 'Resolving existing GitHub workflow %s run for %s...\n' "$LINUX_WORKFLOW_FILE" "$release_tag"
    linux_run_id="$(wait_for_linux_packages_run "$release_repo_slug" "$release_commit_sha" '1970-01-01T00:00:00Z')"
    download_linux_release_artifacts "$release_repo_slug" "$linux_run_id" "$release_artifact_dir"
    collect_linux_release_assets "$release_artifact_dir"

    if [[ "${#selected_macos_assets[@]}" -gt 0 ]]; then
        release_assets+=("${selected_macos_assets[@]}")
    fi

    create_github_release "$release_repo_slug" "$release_tag" "$notes_file"
    exit 0
fi

ensure_clean_worktree

current_branch="$(git rev-parse --abbrev-ref HEAD)"
[[ "$current_branch" != "HEAD" ]] || die 'Detached HEAD. Check out branch before release.'

upstream_ref="$(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null || true)"
[[ -n "$upstream_ref" ]] || die 'Current branch has no upstream. Set upstream to GitHub branch first.'

upstream_remote="${upstream_ref%%/*}"
upstream_branch="${upstream_ref#*/}"
upstream_remote_url="$(git remote get-url "$upstream_remote" 2>/dev/null || true)"

is_github_remote_url "$upstream_remote_url" || die "Current upstream remote ${upstream_remote} is not GitHub. Set branch upstream to GitHub remote before release."

git fetch "$upstream_remote" --tags >/dev/null

divergence="$(git rev-list --left-right --count "HEAD...${upstream_ref}")"
ahead_count="${divergence%%[[:space:]]*}"
behind_count="${divergence##*[[:space:]]}"
if [[ "$behind_count" != "0" ]]; then
    die "Current branch is behind ${upstream_ref}. Pull or rebase before release."
fi

current_version="$(read_cargo_version)"
validate_semver "$current_version" || die "Current Cargo version is not semantic: ${current_version}"

latest_tag="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
latest_tag_version=''
commit_range='HEAD'
base_version='0.0.0'
if [[ -n "$latest_tag" ]]; then
    latest_tag_version="${latest_tag#v}"
    validate_semver "$latest_tag_version" || die "Latest tag is not semantic: ${latest_tag}"
    commit_range="${latest_tag}..HEAD"
    base_version="$latest_tag_version"
fi

commit_count="$(git rev-list --count --no-merges "$commit_range")"
if [[ "$commit_count" == "0" ]]; then
    if [[ -n "$latest_tag" ]]; then
        die "No non-merge commits since ${latest_tag}. Nothing to release."
    fi
    die 'No non-merge commits to release.'
fi

release_level="$(populate_release_notes_from_commit_range "$commit_range")"

derived_version="$(semver_bump "$base_version" "$release_level")"
suggested_version="$(max_semver "$current_version" "$derived_version")"
release_version="$(prompt_version "$suggested_version" "$latest_tag_version" "$current_version")"
release_tag="v${release_version}"

if git rev-parse -q --verify "refs/tags/${release_tag}" >/dev/null 2>&1; then
    die "Local tag already exists: ${release_tag}"
fi
if git ls-remote --exit-code --tags "$upstream_remote" "refs/tags/${release_tag}" >/dev/null 2>&1; then
    die "Remote tag already exists on ${upstream_remote}: ${release_tag}"
fi

notes_file="$(mktemp "${TMPDIR:-/tmp}/neomist-release-notes.XXXXXX")"
cleanup() {
    rm -f "$notes_file"
}
trap cleanup EXIT

write_release_notes "$notes_file" "$release_version" "$latest_tag" "$commit_count"

printf 'Branch: %s\n' "$current_branch"
printf 'Upstream: %s\n' "$upstream_ref"
printf 'Current Cargo version: %s\n' "$current_version"
if [[ -n "$latest_tag" ]]; then
    printf 'Latest tag: %s\n' "$latest_tag"
fi
printf 'Suggested bump: %s\n' "$release_level"
printf 'Chosen release version: %s\n\n' "$release_version"

cat "$notes_file"
printf '\n'

if ! prompt_yes_no "Proceed with release ${release_tag}?" 'n'; then
    printf 'Release cancelled.\n'
    exit 0
fi

if [[ "$DRY_RUN" == '1' ]]; then
    printf 'Dry run. No files changed.\n'
    if [[ "$release_version" != "$current_version" ]]; then
        printf 'Would update Cargo.toml and Cargo.lock to %s.\n' "$release_version"
        printf 'Would commit: chore(release): %s\n' "$release_tag"
    else
        printf 'Current version already %s. Would skip release version commit.\n' "$release_version"
    fi
    printf 'Would create annotated tag: %s\n' "$release_tag"
    printf 'Would push branch to %s/%s and push tag %s.\n' "$upstream_remote" "$upstream_branch" "$release_tag"
    printf 'Would wait for GitHub workflow %s, download Linux artifacts, and create GitHub release %s.\n' "$LINUX_WORKFLOW_FILE" "$release_tag"
    exit 0
fi

ensure_gh_ready
release_repo_slug="$(resolve_release_repo_slug)"

if [[ "$release_version" != "$current_version" ]]; then
    update_cargo_toml_version "$release_version"
    update_cargo_lock_version "$release_version"
    git add "Cargo.toml" "Cargo.lock"
    git commit -m "chore(release): ${release_tag}"
else
    printf 'Current version already %s. Skipping release version commit.\n' "$release_version"
fi

release_commit_sha="$(git rev-parse HEAD)"
release_started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
git tag -a "$release_tag" -F "$notes_file"
git push "$upstream_remote" "HEAD:refs/heads/${upstream_branch}"
git push "$upstream_remote" "refs/tags/${release_tag}"

printf 'Pushed branch and tag to %s. Tag push should trigger Linux Packages.\n' "$upstream_remote"

macos_pkg_path=''
if [[ "$(uname -s)" == 'Darwin' ]]; then
    if prompt_yes_no 'Build macOS pkg now?' 'n'; then
        sign_pkg=0
        notarize_pkg=0
        sign_default='n'
        if [[ -n "${NEOMIST_APP_SIGN_IDENTITY:-}" && -n "${NEOMIST_INSTALLER_SIGN_IDENTITY:-}" ]]; then
            sign_default='y'
        fi

        if prompt_yes_no 'Sign macOS pkg?' "$sign_default"; then
            sign_pkg=1
        fi

        if prompt_yes_no 'Notarize macOS pkg after build?' 'n'; then
            notarize_pkg=1
            sign_pkg=1
        fi

        if [[ "$sign_pkg" == '1' ]]; then
            [[ -n "${NEOMIST_APP_SIGN_IDENTITY:-}" ]] || die 'Signed macOS pkg requires NEOMIST_APP_SIGN_IDENTITY.'
            [[ -n "${NEOMIST_INSTALLER_SIGN_IDENTITY:-}" ]] || die 'Signed macOS pkg requires NEOMIST_INSTALLER_SIGN_IDENTITY.'
        fi

        if [[ "$notarize_pkg" == '1' ]]; then
            [[ -n "${NEOMIST_NOTARY_PROFILE:-}" ]] || die 'Notarization requires NEOMIST_NOTARY_PROFILE.'
        fi

        macos_args=()
        if [[ "$sign_pkg" == '1' ]]; then
            macos_args+=(--sign)
        fi

        "${ROOT_DIR}/scripts/build-macos-pkg.sh" "${macos_args[@]}"
        if [[ "$notarize_pkg" == '1' ]]; then
            "${ROOT_DIR}/scripts/notarize-macos-pkg.sh"
        fi

        macos_pkg_path="${ROOT_DIR}/dist/neomist-${release_version}-macos-$(uname -m).pkg"
        [[ -f "$macos_pkg_path" ]] || die "Missing built macOS pkg: ${macos_pkg_path}"
    else
        printf 'macOS packaging skipped.\n'
    fi
else
    printf 'macOS packaging skipped: host is %s.\n' "$(uname -s)"
fi

release_artifact_dir="$(mktemp -d "${TMPDIR:-/tmp}/neomist-release-assets.XXXXXX")"
cleanup() {
    rm -f "$notes_file"
    rm -rf "$release_artifact_dir"
}
trap cleanup EXIT

printf 'Waiting for GitHub workflow %s to finish...\n' "$LINUX_WORKFLOW_FILE"
linux_run_id="$(wait_for_linux_packages_run "$release_repo_slug" "$release_commit_sha" "$release_started_at")"
download_linux_release_artifacts "$release_repo_slug" "$linux_run_id" "$release_artifact_dir"
collect_linux_release_assets "$release_artifact_dir"

if [[ -n "$macos_pkg_path" ]]; then
    release_assets+=("$macos_pkg_path")
fi

create_github_release "$release_repo_slug" "$release_tag" "$notes_file"
