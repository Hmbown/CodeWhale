Feature: FEAT-011 Dispatch Precedence And Error Semantics

  Scenario: AT-004 User command shadows built-in canonical name
    Given a workspace with a user command shadowing a built-in canonical name
    When the user runs "/help config"
    Then the user command executes instead of the built-in
    And no built-in /help side effect occurs

  Scenario: AT-005 User command shadows built-in alias
    Given a workspace with a user command shadowing a built-in alias
    When the user runs the shadowed alias
    Then the user command executes
    And the built-in canonical name remains reachable

  Scenario: AT-006 Absent user command falls back to built-in
    Given a workspace with a previously loaded user command
    When the user command file is removed and the command is invoked again
    Then the built-in command executes without a user-command error message

  Scenario: AT-007 Invalid user command produces user error without fallback
    Given a workspace with an invalid user command
    When the user runs the invalid command
    Then a user-command-specific error is returned
    And no built-in fallback occurs
