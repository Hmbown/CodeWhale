# SKILL: Onboarding Suite

Step 0: Environment Check
    - Check if .git/ exists in the current directory
    - Check if .onboarding-init exists

    [Case 1: Empty directory]
        - Prompt user: "This appears to be an empty directory. Would you like to create a testbed here?"
        - If yes → git clone <URL> . && touch .onboarding-init → proceed to Step 1
        - If no → Exit with clear instructions

    [Case 2: Testbed (.git + .codewhale-init present)]
        - ✅ Proceed to Step 1

    [Case 3: Normal repo (.git present, but no .onboarding-init)]
        - Error: "This appears to be a main repository, not a test/onboarding bed/suit."
        - Exit with instructions to create a testbed using git worktree

    [Case 4: Unknown / cluttered]
        - Error: "No valid Git repository or testbed state detected."
        - Exit

Step 1: Update the Testbed
    - Check if the working directory is clean (git status --porcelain)
    - If dirty → Stop with a clear receipt
    - If clean → git pull --ff-only

Step 2: Build and Verify
    - cargo fmt --check
    - cargo clippy -- -D warnings
    - cargo test --workspace

Step 3: Generate Digest
    - Parse the latest entry from CHANGELOG.md
    - Fetch milestone metadata from GitHub
    - Generate a concise "what's new" summary

Step 4: Output Receipt
    - Write receipt to testbed/receipts/latest_receipt.json
    - Include: branch, commit hash, test status, CHANGELOG summary
    - Display report to user

Step 5: Done
    - Output status report


# User wants to use the onboarding suite

# Option 1: They already have a testbed
cd ../codewhale-onboarding
/onboarding-suite   # Skill runs, updates, verifies, digests

# Option 2: They don't have a testbed
cd ~/projects
mkdir codewhale-onboarding
cd codewhale-onboarding
/onboarding-suite   # Skill sees empty dir, prompts to clone and init

# Option 3: They're in the main repo (by accident?)
cd codewhale
/onboarding-suite   # Skill warns: "This is the main repo, not a testbed."
                    # "Recommended: git worktree add ../codewhale-onboarding main"
                    # "Then cd ../codewhale-onboarding and run again."

# Architecture Draft

.codewhale/
├── commands/
│   ├── surf.md                 # /surf  (orchestrator)
│   └── surf-setup.md           # /surf setup  (clone/init)
├── skills/
│   └── surf/
│       ├── SKILL.md            # $surf  (LLM-enhanced)
│       └── scripts/
│           ├── surf.sh         # MAIN ORCHESTRATOR
│           ├── check-wave.sh
│           ├── catch-wave.sh
│           └── ride-wave.sh
└── config.toml