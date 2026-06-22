Feature: EPIC acceptance harness

  Scenario: Gherkin acceptance tests can run on the target branch
    Given the acceptance harness is available
    When the runner discovers EPIC scenarios
    Then the runner exits successfully
