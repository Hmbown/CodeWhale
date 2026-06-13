Feature: Directory listing acceptance
  Scenario: Happy path lists a workspace directory
    Given an offline CodeWhale evaluation workspace
    When the user asks "list the current directory"
    Then the simulated LLM should call the "list_dir" tool
    And the tool output should include:
      | entry     |
      | README.md |
      | notes.txt |
      | src       |
