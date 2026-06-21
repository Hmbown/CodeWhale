Feature: Core and session command extraction

  Scenario: The binary loads and runs the evaluation harness after extraction
    Given a clean CodeWhale evaluation workspace
    When the evaluation harness runs a shell command
    Then the harness completes successfully
    And the JSON report contains a step with the expected kind
