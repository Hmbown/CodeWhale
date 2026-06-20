import re

file_path = r'C:\project\F_project1\CodeWhale\crates\tui\src\core\engine.rs'
with open(file_path, 'r', encoding='utf-8') as f:
    content = f.read()

# 1. Add helper function after runtime_prompt_text
marker = '         </runtime_prompt>"\n    )\n}\n\n/// Spawn the engine'
helper_fn = '''         </runtime_prompt>"
    )
}

/// Check if a user message contains real user input (not just runtime metadata).
/// Returns true if the message has actual user text content beyond internal tags.
fn has_real_user_content(text: &str) -> bool {
    // Strip known internal tags and check if meaningful content remains
    let stripped = text
        .replace("<turn_meta>", "")
        .replace("</turn_meta>", "")
        .replace("<runtime_prompt", "")
        .replace("</runtime_prompt>", "")
        .replace("<codewhale:runtime_event", "")
        .replace("</codewhale:runtime_event>", "");

    // Check if there's non-whitespace content after stripping tags
    let trimmed = stripped.trim();
    !trimmed.is_empty() && trimmed.len() > 10 // Allow for minimal metadata
}

/// Spawn the engine'''

if marker in content:
    content = content.replace(marker, helper_fn)
    print("OK: helper function added")
else:
    print("WARN: marker not found for helper function")

with open(file_path, 'w', encoding='utf-8') as f:
    f.write(content)
