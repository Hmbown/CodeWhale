@long-running
# [LONG RUNNING] Opt-in core command acceptance workflows. Run with:
# cargo test -p codewhale-tui --bin codewhale-tui --features long-running-tests commands::groups::core::acceptance -- --test-threads=1
Feature: Core command visible surfaces

  Scenario: Core informational commands write visible transcript messages
    Given a CodeWhale core command workspace
    When the user runs the core command "/help links"
    Then the message window should include "Usage: /links"
    And the message window should include "Aliases: dashboard, api"
    When the user runs the core command "/links"
    Then the message window should include "https://platform.deepseek.com"
    When the user runs the core command "/workspace"
    Then the message window should include "Current workspace:"
    When the user runs the core command "/home"
    Then the message window should include "codewhale Home Dashboard"
    And the message window should include "/links"

  Scenario: Core state commands report visible changes
    Given a CodeWhale core command workspace
    When the user runs the core command "/model auto"
    Then the message window should include "Model changed:"
    And the message window should include "auto"
    When the user runs the core command "/translate"
    Then the message window should include "Output translation enabled"
    When the user runs the core command "/translate"
    Then the message window should include "Output translation disabled"

  Scenario: Clear replaces prior transcript with visible confirmation
    Given a CodeWhale core command workspace with one visible user message
    When the user runs the core command "/clear"
    Then the message window should include "Conversation cleared"
    And the message window should not include "Remember the whale migration"

  Scenario: Persistent work commands report visible dispatch requests
    Given a CodeWhale core command workspace
    When the user runs the core command "/agent 2 summarize logs"
    Then the message window should include "Opening persistent sub-agent at depth 2"
    When the user runs the core command "/rlm 1 inspect command extraction"
    Then the message window should include "Opening persistent RLM context at depth 1"
    When the user runs the core command "/swarm 2 audit commands"
    Then the message window should include "/swarm is gated"
