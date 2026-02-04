#!/bin/bash

# Script to generate commit message using Claude Code CLI
# Follows Commitizen conventional commit specification

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

# Emojis
ROCKET="🚀"
CHECK="✅"
CROSS="❌"
WARN="⚠️"
SPARKLES="✨"
GEAR="⚙️"
PENCIL="📝"
MAGNIFY="🔍"
PACKAGE="📦"

# Stage all changes
git add -A

# Get git changes
CHANGES=$(git status --short)

# Check if there are any changes
if [ -z "$CHANGES" ]; then
    echo -e "${DIM}No changes to commit.${RESET}"
    exit 0
fi

# Show what will be committed
echo -e "\n${BOLD}${BLUE}${MAGNIFY} Changes to be committed${RESET}"
echo -e "${DIM}────────────────────────────────────────${RESET}"
git diff --cached --stat
echo ""

# Use Claude Code CLI to generate the message
echo -e "${BOLD}${MAGENTA}${SPARKLES} Generating commit message with AI...${RESET}"

# Count files
FILE_COUNT=$(echo "$CHANGES" | wc -l | xargs)

# Commitizen conventional commit types
CZ_TYPES="feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert"

# Get file change summary
FILE_CHANGES=$(git diff --cached --name-status | head -20)

# Build prompt following Commitizen specification
PROMPT="You are a commit message generator. Analyze these changes and write ONE LINE commit message in Commitizen format.

File changes ($FILE_COUNT files):
$FILE_CHANGES

REQUIRED FORMAT: <type>(<scope>): <subject>

Types (pick ONE): feat, fix, docs, style, refactor, perf, test, build, ci, chore

STRICT RULES:
1. Output ONLY ONE LINE - the commit message itself
2. NO explanations, NO markdown, NO extra text
3. Use imperative mood: \"add\" NOT \"added\" or \"adds\"
4. Subject in lowercase (not title case)
5. No period at end
6. Keep under 72 characters
7. Scope is optional but recommended

EXAMPLES (output format):
feat(api): add user authentication endpoint
fix: resolve memory leak in parser
docs(readme): update installation instructions
chore: upgrade dependencies to latest versions
feat(matrix): add shared UI component library

CRITICAL: Respond with ONLY the commit message line. Nothing else."

# Show loading indicator with modern spinner
show_loading() {
    local pid=$1
    local delay=0.08
    local spinstr='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'
    local temp

    while kill -0 $pid 2>/dev/null; do
        for i in $(seq 0 9); do
            if ! kill -0 $pid 2>/dev/null; then
                break 2
            fi
            local char=${spinstr:$i:1}
            printf "\r  ${CYAN}${char}${RESET} Analyzing changes..."
            sleep $delay
        done
    done
    printf "\r${GREEN}${CHECK}${RESET} Analysis complete     \n"
}

# Try to get AI-generated message
claude -p --print --model haiku "$PROMPT" > /tmp/commit_msg.txt 2>&1 &
CLAUD_PID=$!

# Show loading animation
show_loading $CLAUD_PID

# Wait for result
wait $CLAUD_PID 2>/dev/null

# Extract commit message - take first non-empty line that matches commit format
COMMIT_MSG=$(cat /tmp/commit_msg.txt 2>/dev/null | grep -E '^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\(.+?\))?:.+' | head -1 | xargs)

# If no match, try first non-empty line
if [ -z "$COMMIT_MSG" ]; then
    COMMIT_MSG=$(cat /tmp/commit_msg.txt 2>/dev/null | grep -v '^$' | head -1 | xargs)
fi

rm -f /tmp/commit_msg.txt

# Validate we got a message
if [ -z "$COMMIT_MSG" ] || [[ "$COMMIT_MSG" == *"Error"* ]] || [[ "$COMMIT_MSG" == *"error"* ]]; then
    echo -e "${YELLOW}${WARN}  AI generation failed${RESET}"
    echo ""
    echo -e "${DIM}Please provide a Commitizen-compliant commit message:${RESET}"
    echo -e "${DIM}Format: ${BOLD}<type>(<scope>): <subject>${RESET}"
    echo -e "${DIM}Types: $CZ_TYPES${RESET}"
    echo ""
    read -r COMMIT_MSG
fi

echo ""
echo -e "${BOLD}${CYAN}${PENCIL} Generated commit message${RESET}"
echo -e "${DIM}────────────────────────────────────────${RESET}"
echo -e "${BOLD}${GREEN}$COMMIT_MSG${RESET}"
echo -e "${DIM}────────────────────────────────────────${RESET}"
echo ""

# Ask for confirmation
echo -e "${BOLD}Accept this message?${RESET}"
echo -e "  ${GREEN}[y]${RESET} Yes, commit with this message"
echo -e "  ${RED}[n]${RESET} No, cancel"
echo -e "  ${YELLOW}[e]${RESET} Edit the message"
echo ""
read -p "Your choice (y/n/e): " -n 1 -r
echo
echo ""

if [[ $REPLY =~ ^[Nn]$ ]]; then
    echo -e "${RED}${CROSS} Commit cancelled${RESET}"
    exit 1
elif [[ $REPLY =~ ^[Ee]$ ]]; then
    echo -e "${YELLOW}${PENCIL} Enter custom commit message (Commitizen format):${RESET}"
    echo -e "${DIM}Format: <type>(<scope>): <subject>${RESET}"
    echo ""
    read -r COMMIT_MSG
    echo ""
fi

# Function to generate CHANGELOG entry
generate_changelog_entry() {
    local commit_msg="$1"

    # Extract type from commit message (e.g., "feat", "fix", etc.)
    local commit_type=$(echo "$commit_msg" | sed -n 's/^\([a-z]*\).*/\1/p')

    # Extract scope and subject
    local scope=$(echo "$commit_msg" | sed -n 's/^[a-z]*(\([^)]*\)).*/\1/p')
    local subject=$(echo "$commit_msg" | sed -n 's/^[a-z]*\(([^)]*)\)\?:\s*\(.*\)/\2/p')

    # Map commit type to CHANGELOG category
    local category=""
    case "$commit_type" in
        feat) category="Added" ;;
        fix) category="Fixed" ;;
        perf) category="Changed" ;;
        refactor) category="Changed" ;;
        docs) category="Changed" ;;
        style) category="Changed" ;;
        test) category="Changed" ;;
        build) category="Changed" ;;
        ci) category="Changed" ;;
        chore) category="Changed" ;;
        revert) category="Changed" ;;
        *) category="Changed" ;;
    esac

    # Get detailed file changes
    local file_changes=$(git diff --cached --name-status | head -10)

    # Build prompt for CHANGELOG entry
    local changelog_prompt="Generate a concise CHANGELOG entry for this commit.

Commit message: $commit_msg
Category: $category

File changes:
$file_changes

Detailed changes:
$(git diff --cached --stat)

Rules:
- Write ONE concise bullet point describing what changed
- Focus on WHAT changed from user perspective, not HOW
- Start with a verb (Add, Fix, Update, Remove, etc.)
- Be specific but brief (one line)
- If multiple files changed, summarize the overall change
- Do NOT include technical implementation details
- Do NOT mention file names unless critical

Examples:
- Add user authentication with JWT tokens
- Fix memory leak in background worker process
- Update API documentation for v2 endpoints
- Remove deprecated configuration options

Generate ONLY the bullet point text (without the dash), nothing else."

    # Generate CHANGELOG entry using Claude
    echo -e "${BOLD}${MAGENTA}${PACKAGE} Generating CHANGELOG entry...${RESET}"

    # Run in background to show spinner
    claude -p --print --model haiku "$changelog_prompt" > /tmp/changelog_entry.txt 2>&1 &
    local claude_pid=$!

    # Simple spinner for changelog
    local delay=0.08
    local spinstr='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'
    while kill -0 $claude_pid 2>/dev/null; do
        for i in $(seq 0 9); do
            if ! kill -0 $claude_pid 2>/dev/null; then
                break 2
            fi
            local char=${spinstr:$i:1}
            printf "\r  ${CYAN}${char}${RESET} Writing changelog..."
            sleep $delay
        done
    done
    printf "\r${GREEN}${CHECK}${RESET} Changelog generated   \n"

    wait $claude_pid 2>/dev/null
    local changelog_entry=$(cat /tmp/changelog_entry.txt 2>/dev/null | head -1 | xargs)
    rm -f /tmp/changelog_entry.txt

    if [ -z "$changelog_entry" ]; then
        # Fallback: use commit subject
        changelog_entry="$subject"
        if [ -n "$scope" ]; then
            changelog_entry="$scope: $subject"
        fi
    fi

    echo -e "${DIM}Entry: ${RESET}${changelog_entry}"
    echo ""

    # Update CHANGELOG.md
    if [ -f "CHANGELOG.md" ]; then
        # Create temporary file
        local temp_file=$(mktemp)

        # Read CHANGELOG and insert entry under [Unreleased]
        local in_unreleased=0
        local category_found=0
        local inserted=0

        while IFS= read -r line; do
            echo "$line" >> "$temp_file"

            # Detect [Unreleased] section
            if [[ "$line" =~ ^\#\#[[:space:]]*\[Unreleased\] ]]; then
                in_unreleased=1
                continue
            fi

            # Exit [Unreleased] when hitting next version
            if [ $in_unreleased -eq 1 ] && [[ "$line" =~ ^\#\#[[:space:]]*\[[0-9] ]]; then
                in_unreleased=0
            fi

            # Look for category in [Unreleased]
            if [ $in_unreleased -eq 1 ] && [ $inserted -eq 0 ]; then
                if [[ "$line" =~ ^\#\#\#[[:space:]]*$category ]]; then
                    category_found=1
                    echo "- $changelog_entry" >> "$temp_file"
                    inserted=1
                elif [ $category_found -eq 0 ] && [[ "$line" =~ ^\#\#\#[[:space:]]* ]] && [[ ! "$line" =~ ^\#\#\#[[:space:]]*$category ]]; then
                    # Insert new category before other categories
                    echo "" >> "$temp_file"
                    echo "### $category" >> "$temp_file"
                    echo "- $changelog_entry" >> "$temp_file"
                    echo "$line" >> "$temp_file"
                    inserted=1
                    category_found=1
                    continue
                fi
            fi
        done < CHANGELOG.md

        # If category not found and still in unreleased, append at the end
        if [ $in_unreleased -eq 1 ] && [ $inserted -eq 0 ]; then
            echo "" >> "$temp_file"
            echo "### $category" >> "$temp_file"
            echo "- $changelog_entry" >> "$temp_file"
        fi

        # Replace original CHANGELOG
        mv "$temp_file" CHANGELOG.md

        # Stage CHANGELOG.md
        git add CHANGELOG.md

        echo -e "${GREEN}${CHECK} CHANGELOG.md updated${RESET}"
        echo ""
    else
        echo -e "${YELLOW}${WARN} CHANGELOG.md not found, skipping...${RESET}"
        echo ""
    fi
}

# Generate and update CHANGELOG
generate_changelog_entry "$COMMIT_MSG"

# Commit with the message
echo -e "${BOLD}${BLUE}${GEAR} Committing changes...${RESET}"
if git commit -m "$COMMIT_MSG"; then
    echo ""
    echo -e "${GREEN}${BOLD}${ROCKET} Changes committed successfully!${RESET}"
    echo ""
    echo -e "${DIM}Commit message: ${RESET}${BOLD}$COMMIT_MSG${RESET}"
    echo ""
    exit 0
else
    echo ""
    echo -e "${RED}${CROSS} Commit failed${RESET}"
    exit 1
fi
