#!/usr/bin/env node

const notice = [
  "",
  "  ╭───────────────────────────────────────────────────────────────────╮",
  "  │                                                                   │",
  "  │  deepseek-tui has been renamed to `codewhale`.                    │",
  "  │                                                                   │",
  "  │  Please uninstall this package and install codewhale instead:     │",
  "  │                                                                   │",
  "  │    npm uninstall -g deepseek-tui                                  │",
  "  │    npm install -g codewhale                                       │",
  "  │                                                                   │",
  "  │  codewhale installs the `codewhale` and `codew` commands.         │",
  "  │  Historical old-name shims ended with v0.8.x. See:                │",
  "  │  https://github.com/Hmbown/CodeWhale/blob/main/docs/REBRAND.md │",
  "  │                                                                   │",
  "  ╰───────────────────────────────────────────────────────────────────╯",
  "",
].join("\n");

process.stderr.write(notice);
