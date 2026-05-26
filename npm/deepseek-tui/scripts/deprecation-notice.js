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
  "  │  codewhale ships the same `codewhale` and `codewhale-tui`         │",
  "  │  binaries plus deprecation shims under the old names. See:        │",
  "  │  https://github.com/Hmbown/CodeWhale/blob/main/docs/REBRAND.md │",
  "  │                                                                   │",
  "  ╰───────────────────────────────────────────────────────────────────╯",
  "",
].join("\n");

process.stderr.write(notice);
