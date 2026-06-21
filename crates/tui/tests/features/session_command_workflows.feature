@long-running
# [LONG RUNNING] Opt-in acceptance workflows. Run with:
# cargo test -p codewhale-tui --bin codewhale-tui --features long-running-tests commands::groups::session::acceptance -- --test-threads=1
Feature: Session command workflows

  Scenario: Save, export, and load preserve the active session
    Given a CodeWhale session workspace with one user message
    When the user saves the active session
    And the user exports the active transcript
    And the user clears the active conversation
    And the user loads the saved session
    Then the saved session file should contain the saved message
    And the active session id should match the saved session file
    And the exported markdown should contain the active transcript
    And the active session should contain the saved message
    And the restored token count should match the saved session
    And CodeWhale should report that the session was loaded

  Scenario: Fork keeps the original session resumable
    Given a CodeWhale persisted session workspace with one user message
    When the user forks the active session
    Then the forked session should reference the original session
    And the original session should still be loadable
    And the active session should be the forked session

  Scenario: New session cannot be forked before messages exist
    Given a CodeWhale session workspace with one user message
    When the user starts a new session
    And the user tries to fork the active session
    Then CodeWhale should reject the fork because there are no messages
    And the active session should be empty

  Scenario: Cleared session cannot be forked before messages exist
    Given a CodeWhale session workspace with one user message
    When the user clears the active conversation
    And the user tries to fork the active session
    Then CodeWhale should reject the fork because there are no messages
    And the active session should be empty

  Scenario: Fork followed by new keeps both saved sessions
    Given a CodeWhale persisted session workspace with one user message
    When the user forks the active session
    And the user starts a new session
    Then the original and forked sessions should remain loadable
    And the active session should be a new empty session

  Scenario: Fork followed by clear keeps both saved sessions
    Given a CodeWhale persisted session workspace with one user message
    When the user forks the active session
    And the user clears the active conversation
    Then the original and forked sessions should remain loadable
    And the active session should be cleared without an active session id

  Scenario: Rename updates the active saved session title
    Given a CodeWhale persisted session workspace with one user message
    When the user renames the active session to "Renamed whale path"
    Then the active saved session title should be "Renamed whale path"
    And the active session should be the original session

  Scenario: Sessions list opens the saved session picker
    Given a CodeWhale persisted session workspace with one user message
    When the user lists saved sessions
    Then the session picker should be open
    And the original session should still be loadable

  Scenario: Sessions prune removes only stale sessions
    Given a CodeWhale session workspace with stale and fresh saved sessions
    When the user prunes sessions older than 7 days
    Then CodeWhale should report that one session was pruned
    And the fresh session should still be loadable
    And the stale session should no longer be loadable

  Scenario: Context management commands emit actions without clearing the active session
    Given a CodeWhale session workspace with one user message
    When the user compacts context
    Then CodeWhale should trigger context compaction
    And the active session should contain the saved message
    When the user purges context
    Then CodeWhale should trigger context purge
    And the active session should contain the saved message
    When the user prepares a session relay focused on "handoff details"
    Then CodeWhale should send a session relay instruction focused on "handoff details"
    And the active session should contain the saved message

  Scenario: Singular session command is not registered
    Given a CodeWhale session workspace with one user message
    When the user runs the singular session command
    Then CodeWhale should reject the unknown session command
    And the active session should contain the saved message
