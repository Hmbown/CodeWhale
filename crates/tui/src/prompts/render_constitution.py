#!/usr/bin/env python3
"""
Render the CodeWhale constitution from YAML to the markdown format
the engine currently expects (equivalent to prompts/base.md output).

Usage:
    python3 render_constitution.py [--yaml constitution.yaml] [--model deepseek-v4-pro]

The YAML structure uses indentation to encode precedence:
  - tier 1 (constitution) is at top level
  - tier 3 (statutes) is nested under statutes
  - tier 4 (regulations) is nested under regulations
  - etc.

This renderer flattens the YAML into the current flat-markdown format
that the engine's prompt assembly pipeline expects.
"""

import sys
import yaml
from pathlib import Path


def indent(text: str, spaces: int = 4) -> str:
    """Indent every line of text by `spaces` spaces."""
    prefix = " " * spaces
    return "\n".join(prefix + line if line else "" for line in text.split("\n"))


def bullet_list(items: list, level: int = 0) -> str:
    """Render a list of strings as markdown bullets."""
    prefix = "  " * level
    return "\n".join(f"{prefix}- {item}" for item in items)


def numbered_list(items: list) -> str:
    """Render a list of strings as a numbered markdown list."""
    return "\n".join(f"{i}. {item}" for i, item in enumerate(items, 1))


def render_constitution(data: dict, model_id: str = "codewhale") -> str:
    """Convert the YAML constitution into markdown."""
    out = []

    # ── Preamble ──
    preamble = data.get("preamble", "")
    out.append(preamble.replace("{model_id}", model_id).strip())
    out.append("")

    # ── Constitution (Tier 1) ──
    const = data.get("constitution", {})

    # Article I
    a1 = const.get("article_1_identity", {})
    out.append("### Article I — The Identity of the Agent")
    out.append("")
    out.append(a1.get("text", "").strip())
    out.append("")
    for rule in a1.get("rules", []):
        out.append(rule)
    out.append("")

    # Article II
    a2 = const.get("article_2_truth", {})
    out.append("### Article II — The Primacy of Truth")
    out.append("")
    out.append(a2.get("text", "").strip())
    out.append("")
    if a2.get("non_negotiable"):
        out.append(f"This Article is non-negotiable. {a2.get('note', '')}")
    out.append("")

    # Article III
    a3 = const.get("article_3_user_agency", {})
    out.append("### Article III — The Agency of the User")
    out.append("")
    out.append(a3.get("text", "").strip())
    out.append("")
    for g in a3.get("guidance", []):
        out.append(g)
    out.append("")

    # Article IV
    a4 = const.get("article_4_action", {})
    out.append("### Article IV — The Duty of Action")
    out.append("")
    out.append(a4.get("text", "").strip())
    out.append("")

    # Article V
    a5 = const.get("article_5_verification", {})
    out.append("### Article V — The Discipline of Verification")
    out.append("")
    out.append(a5.get("text", "").strip())
    out.append("")

    # Article VI
    a6 = const.get("article_6_legacy", {})
    out.append("### Article VI — The Legacy of Coordination")
    out.append("")
    out.append(a6.get("text", "").strip())
    out.append("")
    deeper = a6.get("deeper", "")
    if deeper:
        out.append(deeper.strip())
    out.append("")

    # Article VII — Hierarchy
    a7 = const.get("article_7_hierarchy", {})
    out.append("### Article VII — The Hierarchy of Law")
    out.append("")
    out.append(a7.get("text", "").strip())
    out.append("")
    for level in a7.get("levels", []):
        out.append(f"{level['tier']}. **{level['name']}.** {level.get('note', '')}")
    out.append("")

    out.append("---")
    out.append("")

    # ── Statutes (Tier 3) ──
    statutes = data.get("statutes", {})
    out.append("## STATUTES (Tier 2)")
    out.append("")

    lang = statutes.get("language", {})
    out.append("## Language")
    out.append("")
    out.append(lang.get("text", "").strip())
    out.append("")
    if lang.get("override_rule"):
        out.append(lang["override_rule"].strip())
        out.append("")
    for g in lang.get("guidance", []):
        out.append(g)
        out.append("")
    out.append("")

    fmt = statutes.get("output_formatting", {})
    out.append("## Output Formatting")
    out.append("")
    out.append(fmt.get("text", "").strip())
    out.append("")
    if fmt.get("table_rule"):
        out.append(fmt["table_rule"].strip())
    out.append("")

    vp = statutes.get("verification_principle", {})
    out.append("## Verification Principle")
    out.append("")
    out.append(vp.get("text", "").strip())
    out.append("")
    for check in vp.get("checks", []):
        out.append(f"- **{check.split(':')[0]}**: {':'.join(check.split(':')[1:]).strip()}" if ':' in check else f"- {check}")
    out.append("")
    for rule in vp.get("rules", []):
        out.append(rule)
    out.append("")

    ed = statutes.get("execution_discipline", {})
    out.append("## Execution Discipline (Tier 2 Statute)")
    out.append("")
    tp = ed.get("tool_persistence", [])
    if tp:
        out.append("<tool_persistence>")
        out.append(bullet_list(tp))
        out.append("</tool_persistence>")
        out.append("")
    out.append("<mandatory_tool_use>")
    out.append(ed.get("mandatory_tool_use", "").strip())
    out.append("</mandatory_tool_use>")
    out.append("")
    out.append("<act_dont_ask>")
    out.append(ed.get("act_dont_ask", "").strip())
    out.append("</act_dont_ask>")
    out.append("")
    out.append("<verification>")
    out.append(ed.get("verify_changes", "").strip())
    out.append("</verification>")
    out.append("")
    out.append("<missing_context>")
    out.append(ed.get("missing_context", "").strip())
    out.append("</missing_context>")
    out.append("")

    tue = statutes.get("tool_use_enforcement", {})
    out.append("## Tool-use enforcement")
    out.append("")
    out.append(tue.get("text", "").strip())
    out.append("")

    out.append("---")
    out.append("")

    # ── Regulations (Tier 4) ──
    regs = data.get("regulations", {})
    out.append("## REGULATIONS (Tier 3)")
    out.append("")

    comp = regs.get("composition", {})
    out.append("## Composition Pattern for Multi-Step Work")
    out.append("")
    out.append(comp.get("text", "").strip())
    out.append("")
    for i, step in enumerate(comp.get("steps", []), 1):
        out.append(f"{i}. {step}")
    out.append("")

    sub = regs.get("sub_agent_strategy", {})
    out.append("## Sub-Agent Strategy")
    out.append("")
    out.append(sub.get("text", "").strip())
    out.append("")
    for pattern in sub.get("patterns", []):
        out.append(f"- {pattern}")
    out.append("")

    pf = regs.get("parallel_first", {})
    out.append("## Parallel-First Heuristic")
    out.append("")
    out.append(pf.get("text", "").strip())
    out.append("")

    rlm = regs.get("rlm_usage", {})
    out.append("## RLM — How to Use It")
    out.append("")
    out.append(rlm.get("text", "").strip())
    out.append("")
    for pattern in rlm.get("patterns", []):
        out.append(f"**{pattern.split(' — ')[0]}** — {' — '.join(pattern.split(' — ')[1:])}" if ' — ' in pattern else f"- {pattern}")
    out.append("")
    for rule in rlm.get("rules", []):
        out.append(f"- {rule}")
    out.append("")

    cm = regs.get("context_management", {})
    out.append("## Context Management")
    out.append("")
    out.append(cm.get("text", "").strip())
    out.append("")
    for v4 in cm.get("v4_characteristics", []):
        out.append(f"- {v4}")
    out.append("")

    tb = regs.get("thinking_budget", {})
    out.append("## Thinking Budget")
    out.append("")
    out.append(tb.get("text", "").strip())
    out.append("")
    out.append("| Task type | Thinking depth | Rationale |")
    out.append("|-----------|---------------|-----------|")
    for item in tb.get("levels", []):
        out.append(f"| {item['task']} | {item['depth']} | |")
    out.append("")

    out.append("---")
    out.append("")

    # ── Evidence (Tier 6) ──
    ev = data.get("evidence", {})
    out.append("## EVIDENCE (Tier 6)")
    out.append("")

    toolbox = ev.get("toolbox", {})
    out.append("## Toolbox (fast reference — tool descriptions are authoritative)")
    out.append("")
    for category, tools in toolbox.items():
        label = category.replace("_", " ").title()
        tool_str = ", ".join(f"`{t}`" for t in tools if not t.startswith("gh "))
        if label == "Github":
            tool_str = ", ".join(t for t in tools)
        out.append(f"- **{label}**: {tool_str}")
    out.append("")

    ts = ev.get("tool_selection", {})
    out.append("## Tool Selection Guide")
    out.append("")
    for name, desc in ts.items():
        full_name = name.replace("_", " ").title()
        out.append(f"### `{name}`")
        out.append(desc.strip())
        out.append("")

    sdp = ev.get("subagent_done_protocol", {})
    out.append("## Internal Sub-agent Completion Events")
    out.append("")
    out.append(sdp.get("text", "").strip())
    out.append("")

    out.append("---")
    out.append("")

    # ── Compaction Relay (Tier 9) ──
    cr = data.get("compaction_relay", {})
    if cr.get("conditional"):
        out.append("<!-- COMPACTION_RELAY_PLACEHOLDER -->")
        out.append("")
        out.append("## Compaction Relay — Tier 9 (Precedent)")
        out.append("")
        out.append("The conversation above this point has been compacted.")
        out.append("Below is a structured summary of what was discussed and decided.")
        out.append("")
        for key in ["goal", "constraints"]:
            val = cr.get("template", {}).get(key, "")
            title = key.replace("_", " ").title()
            out.append(f"### {title}")
            out.append(val)
            out.append("")
        progress = cr.get("template", {}).get("progress", {})
        if progress:
            out.append("### Progress")
            out.append("")
            for subkey in ["done", "in_progress", "blocked"]:
                val = progress.get(subkey, "")
                title = subkey.replace("_", " ").title()
                out.append(f"#### {title}")
                out.append(val)
                out.append("")
        for key in ["key_decisions", "next_step"]:
            val = cr.get("template", {}).get(key, "")
            title = key.replace("_", " ").title()
            out.append(f"### {title}")
            out.append(val)
            out.append("")
        out.append(cr.get("template", {}).get("staleability", "").strip())

    out.append("")
    out.append("---")
    out.append("")

    # ── Authority Recap ──
    recap = data.get("authority_recap", {}).get("text", "")
    out.append("## Authority Recap")
    out.append("")
    out.append(recap.strip())

    return "\n".join(out)


def main():
    yaml_path = Path(__file__).parent / "constitution.yaml"
    model_id = "codewhale"

    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--yaml" and i + 1 < len(args):
            yaml_path = Path(args[i + 1])
            i += 2
        elif args[i] == "--model" and i + 1 < len(args):
            model_id = args[i + 1]
            i += 2
        else:
            i += 1

    if not yaml_path.exists():
        print(f"Error: {yaml_path} not found", file=sys.stderr)
        sys.exit(1)

    with open(yaml_path) as f:
        data = yaml.safe_load(f)

    rendered = render_constitution(data, model_id)
    print(rendered)

    # Stats
    import re
    words = len(re.findall(r'\S+', rendered))
    lines = rendered.count('\n') + 1
    print(f"\n<!-- Stats: {lines} lines, ~{words} words -->", file=sys.stderr)


if __name__ == "__main__":
    main()
