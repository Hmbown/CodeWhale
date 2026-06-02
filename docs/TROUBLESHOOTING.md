# Multi-Tab System Troubleshooting

Common issues and solutions for the multi-tab and cross-tab collaboration features.

## Tab Bar Not Showing

**Symptom**: Press `Ctrl+Shift+N` to create a new tab, but no tab bar appears at the top of the screen.

**Cause**: The tab bar only renders when 2 or more tabs exist. A single tab is implied and the bar would be visual noise.

**Solution**: Create a second tab with `Ctrl+Shift+N`. The bar will appear automatically.

**Verify**: After creating 2 tabs, the top row should show:
```
[💬 1:Tab1] [💬 2:Tab2*]
```
The `*` marks the active tab.

---

## Keyboard Shortcuts Not Working

**Symptom**: `Ctrl+Tab`, `Ctrl+\``, or other tab shortcuts don't switch tabs.

**Cause**: The composer is focused and is consuming the keystroke (e.g., `Tab` triggers slash-command completion in the composer).

**Solution**: 
- For `Ctrl+\`` (switcher): Should always work, even with composer focused.
- For `Ctrl+Tab`/`Ctrl+Shift+Tab`: Should also work, but on some terminals the key sequence is captured by the OS or terminal emulator.
- For `Ctrl+1..9`: Same caveat as above.

**Verify**: Try `Ctrl+\`` first — it always opens the tab switcher overlay regardless of context.

---

## "Max Tabs Reached" Status

**Symptom**: Trying to create a new tab shows "Max tabs reached" in the status bar.

**Cause**: There is a hard limit of 9 tabs per window.

**Solution**: Close an existing tab with `Ctrl+Shift+W` to free up a slot. Currently there is no way to exceed 9 tabs in a single window.

**Note**: The 9-tab limit is shared across all TabType variants (Chat, Delegation, Review, Meeting).

---

## Delegated Task Not Showing

**Symptom**: After delegating a task to another tab, the target tab doesn't show the task.

**Cause**: The target tab isn't focused yet. Delegated tasks are queued until the target tab is active and the next user message is dispatched.

**Solution**:
1. Switch to the target tab (use `Ctrl+`` to see all tabs)
2. Type any message and press Enter (or use `Ctrl+Shift+D` to manually process)
3. The task should appear as a System message: "[Delegated Task from Tab #1] Priority: HIGH\n<description>"

**Verify**: 
- Status bar should show: "Processing delegated task delegation_3 (from Tab #1)"
- History should contain a new System cell with the task content

---

## Tab State Not Persisted

**Symptom**: After restarting the app, all my tabs are gone.

**Cause**: Tab state is saved to `~/.codewhale/tabs.json`. If this file is missing, corrupted, or unwritable, tabs are not restored.

**Solutions**:

1. **Check the file exists**:
   ```bash
   ls -la ~/.codewhale/tabs.json
   ```

2. **Check file permissions**:
   ```bash
   chmod 600 ~/.codewhale/tabs.json
   ```

3. **If the file is corrupted**, delete it (you'll lose the tab list but messages are stored elsewhere):
   ```bash
   rm ~/.codewhale/tabs.json
   ```

4. **Check disk space**:
   ```bash
   df -h ~
   ```

**Note**: Only tab metadata is persisted. Conversation history lives in the session files (separate).

---

## Tab Group Color Not Showing

**Symptom**: I assigned a tab to a group but no color appears in the tab bar.

**Cause**: 
1. The tab is in a group but you're using a terminal without true-color support
2. The terminal is using a low-color mode

**Solution**: 
- Use a modern terminal (iTerm2, Windows Terminal, GNOME Terminal, WezTerm) with true-color enabled
- Check the terminal's color settings (look for "true color" or "24-bit color")
- The tab will still show the group tag `⟨Bl⟩` even without color

---

## Context Menu Collaboration Items Missing

**Symptom**: Right-click doesn't show "Delegate to tab...", "Invite to meeting..." options.

**Cause**: Collaboration menu items only appear when 2 or more tabs exist.

**Solution**: Create a second tab with `Ctrl+Shift+N`, then right-click again.

---

## Ctrl+Shift+D Shows "No pending delegations"

**Symptom**: Pressing `Ctrl+Shift+D` shows "No pending delegations" even though I just delegated something.

**Cause**: 
1. You delegated to a different tab — the current tab has no pending delegations
2. The task was already processed

**Solution**: Switch to the target tab (using `Ctrl+\`` to see all tabs) and press `Ctrl+Shift+D` there.

---

## Performance: Too Many Completed Delegations

**Symptom**: Memory usage grows over time when many delegations are completed.

**Status**: This is already mitigated. The delegator now auto-prunes to keep at most 256 completed results (VecDeque with bounded size).

**Verify**: Look at the test `test_auto_prune_bounded_results` which verifies this behavior.

---

## Where Are Tabs Stored on Disk?

**Location**: `~/.codewhale/tabs.json` (Unix) or `%USERPROFILE%\.codewhale\tabs.json` (Windows)

**Format**: JSON with the following structure:
```json
{
  "version": 1,
  "saved_at": "2026-06-01T12:00:00Z",
  "active_tab_index": 0,
  "tabs": [
    {
      "id": 1,
      "title": "Tab 1",
      "tab_type": "Chat",
      "created_at": "...",
      "last_active": "..."
    }
  ],
  "delegations": []
}
```

**Manual editing**: You can edit this file to rename tabs or change tab types, but it's not recommended. Closing the app cleanly is the safe way to update it.

---

## Reporting Issues

If you encounter a problem not covered here:

1. Check the [KEYBINDINGS.md](./KEYBINDINGS.md) and [ARCHITECTURE.md](./ARCHITECTURE.md) docs
2. Search existing issues on GitHub
3. Open a new issue with:
   - Your terminal type and OS
   - Steps to reproduce
   - Expected vs actual behavior
   - Contents of `~/.codewhale/tabs.json` (if relevant, redact sensitive info)
