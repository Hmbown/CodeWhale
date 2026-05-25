# Compact GENmicon Context

Preserve only derived play context needed for the next turn. The authoritative
state remains `STATE.json` and `TURN_LOG.jsonl`; do not summarize transcript
content as a save-state replacement. Keep current scene, open choices, unresolved
warnings, save id, revision, and driver identity.

Never convert compacted transcript text into a state patch. On resume, rebuild
context from the runtime `status` and `render` commands, then treat compacted
context as disposable guidance only.
