# Feature Specification: [FEATURE NAME]

**Feature Branch**: `[###-feature-name]`

**Created**: [DATE]

**Status**: Draft

**Input**: User description: "$ARGUMENTS"

## User Scenarios & Testing *(mandatory)*

<!--
  IMPORTANT: User stories should be PRIORITIZED as user journeys ordered by importance.
  Each user story/journey must be INDEPENDENTLY TESTABLE - meaning if you implement just ONE of them,
  you should still have a viable MVP (Minimum Viable Product) that delivers value.

  Assign priorities (P1, P2, P3, etc.) to each story, where P1 is the most critical.
  Think of each story as a standalone slice of functionality that can be:
  - Developed independently
  - Tested independently
  - Deployed independently
  - Demonstrated to users independently
-->

### User Story 1 - [Brief Title] (Priority: P1)

[Describe this user journey in plain language]

**Why this priority**: [Explain the value and why it has this priority level]

**Independent Test**: [Describe how this can be tested independently - e.g., "Can be fully tested by [specific action] and delivers [specific value]"]

**Acceptance Scenarios**:

1. **Given** [initial state], **When** [action], **Then** [expected outcome]
2. **Given** [initial state], **When** [action], **Then** [expected outcome]

---

### User Story 2 - [Brief Title] (Priority: P2)

[Describe this user journey in plain language]

**Why this priority**: [Explain the value and why it has this priority level]

**Independent Test**: [Describe how this can be tested independently]

**Acceptance Scenarios**:

1. **Given** [initial state], **When** [action], **Then** [expected outcome]

---

### User Story 3 - [Brief Title] (Priority: P3)

[Describe this user journey in plain language]

**Why this priority**: [Explain the value and why it has this priority level]

**Independent Test**: [Describe how this can be tested independently]

**Acceptance Scenarios**:

1. **Given** [initial state], **When** [action], **Then** [expected outcome]

---

[Add more user stories as needed, each with an assigned priority]

### Edge Cases

<!--
  ACTION REQUIRED: The content in this section represents placeholders.
  Fill them out with the right edge cases.
-->

- What happens when a Pi package is missing, disabled, unpinned, filtered, or
  present in both user and project scope?
- How does player mode behave when an extension/tool/package asks for powers
  outside the game-safe active-tool profile?
- What happens when `STATE.json`, `TURN_LOG.jsonl`, or the expected save
  revision is stale, malformed, or missing?
- How does the UI recover when a custom component, renderer, or terminal width
  cannot display the rich game view?
- How are untrusted game markdown, prompt, skill, or package metadata prevented
  from overriding system policy?

## Requirements *(mandatory)*

<!--
  ACTION REQUIRED: The content in this section represents placeholders.
  Fill them out with the right functional requirements.
-->

### Functional Requirements

- **FR-001**: System MUST [specific game/player capability, e.g., "launch a local cartridge from a Pi command or package resource"]
- **FR-002**: System MUST reuse [Pi primitive: package, extension event, command, tool, active-tool policy, session, compaction, provider, renderer, custom UI/editor, skill, prompt, or theme] for [surface]
- **FR-003**: Player mode MUST [hide/show specific game-facing behavior]
- **FR-004**: System MUST persist durable game changes only through [commit path/save file]
- **FR-005**: System MUST prevent [untrusted package/content/tool escalation]
- **FR-006**: Developer mode MUST expose [diagnostics] without changing player-mode trust policy
- **FR-007**: System MUST document and test [command/package key/manifest/tool/save/renderer contract]

*Example of marking unclear requirements:*

- **FR-008**: System MUST load game resources from [NEEDS CLARIFICATION: local Pi package, project .pi settings, npm package, git package, or explicit path?]
- **FR-009**: System MUST use model/provider settings from [NEEDS CLARIFICATION: existing Pi provider registry, custom provider extension, or local adapter?]

### Pi Surface & Trust Requirements *(mandatory)*

- **Pi Primitive**: [Which Pi feature is reused and why it is sufficient]
- **Package Source**: [local path/npm/git/project setting/user setting/N/A, with pin or review status]
- **Loaded Resources**: [extensions/skills/prompts/themes/tools/commands/renderers]
- **Active Tools in Player Mode**: [exact allowlist]
- **Developer-Only Surfaces**: [diagnostics, shell/file/git/provider/package controls]
- **Save Authority**: [which operation writes STATE.json/TURN_LOG.jsonl and how revision conflicts are handled]
- **Untrusted Inputs**: [game markdown, package metadata, issue text, model output, generated skills, etc.]

### Key Entities *(include if feature involves data)*

- **Game Cartridge/Package**: [Manifest, content, skills, prompts, themes, saves, package source]
- **Game Save**: [STATE.json, TURN_LOG.jsonl, revision, driver version, resume summary]
- **Game Tool/Command**: [Pi-registered callable or slash command and its policy]
- **Game View/Renderer**: [Player-facing scene/status/dialogue data and custom UI/renderer contract]
- **Driver Function**: [Deterministic function, declared inputs/outputs, mutating or read-only]

## Success Criteria *(mandatory)*

<!--
  ACTION REQUIRED: Define measurable success criteria.
  These must be technology-agnostic and measurable.
-->

### Measurable Outcomes

- **SC-001**: [Measurable metric, e.g., "Users can complete account creation in under 2 minutes"]
- **SC-002**: [Measurable metric, e.g., "System handles 1000 concurrent users without degradation"]
- **SC-003**: [User satisfaction metric, e.g., "90% of users successfully complete primary task on first attempt"]
- **SC-004**: [Business metric, e.g., "Reduce support tickets related to [X] by 50%"]
- **SC-005**: [Trust metric, e.g., "Player mode exposes only the documented active-tool allowlist in all tested launch paths"]
- **SC-006**: [Persistence metric, e.g., "Restart resumes the same save revision and recent turn state without relying on transcript authority"]

## Assumptions

<!--
  ACTION REQUIRED: The content in this section represents placeholders.
  Fill them out with the right assumptions based on reasonable defaults
  chosen when the feature description did not specify certain details.
-->

- [Assumption about target users, e.g., "Users have stable internet connectivity"]
- [Assumption about scope boundaries, e.g., "Mobile support is out of scope for v1"]
- [Assumption about data/environment, e.g., "Existing authentication system will be reused"]
- [Dependency on existing system/service, e.g., "Requires access to the existing user profile API"]
