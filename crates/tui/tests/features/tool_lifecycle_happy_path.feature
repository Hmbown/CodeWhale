Feature: Tool call lifecycle
  Scenario: Happy path lists the current directory through a tool
    # This executable slice asserts the public exec stream and mocked LLM border.
    # The PTY screen slice should also assert Statusline state and BlueWhale activity:
    # running while the tool is executing, stopped or completed when the turn finishes.
    Given an offline CodeWhale workspace containing:
      | path      | kind   |
      | README.md | file   |
      | notes.txt | file   |
      | src       | folder |
    And the mocked LLM will request the "list_dir" tool with:
      | path |
      | .    |
    And the mocked LLM will answer after the tool result:
      | content                                                    |
      | The directory contains README.md, notes.txt, and src/.      |
    When the user asks "list the current directory"
    Then CodeWhale should send the user request to the mocked LLM
    And the public tool lifecycle should show a running tool:
      | status  | marker | tool     | input |
      | running | [~]    | list_dir | .     |
    And the public tool result should return directory entries:
      | entry     | kind   |
      | README.md | file   |
      | notes.txt | file   |
      | src       | folder |
    And CodeWhale should send the tool result back to the mocked LLM
    And the public tool lifecycle should show a completed tool:
      | status    | marker | tool     | input |
      | completed | ✓      | list_dir | .     |
    And the public output should include "The directory contains README.md, notes.txt, and src/."
