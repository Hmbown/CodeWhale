//! Test-only helpers. Compiled under `#[cfg(all(test, unix))]` only, so
//! nothing here can be reached by a real `codewhale mcp-server` run, and
//! Windows — where the POSIX-sh fixture cannot run and its consumers are
//! `#[cfg(unix)]` — does not compile it as dead code.

use std::path::{Path, PathBuf};

/// A minimal POSIX-sh MCP server used to prove that responses come from a
/// spawned process rather than from an in-process stub.
///
/// It is written in `sh` rather than Rust or Python so the test depends on
/// nothing beyond the shell every unix CI runner already has.
pub const FAKE_MCP_SERVER_SH: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  if [ -z "$id" ]; then
    continue
  fi
  case "$method" in
    initialize)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"fake-mcp","version":"0"}}}\n' "$id"
      ;;
    tools/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"add","description":"add two numbers","inputSchema":{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}}]}}\n' "$id"
      ;;
    tools/call)
      name=$(printf '%s' "$line" | sed -n 's/.*"name":"\([^"]*\)".*/\1/p')
      if [ "$name" = "add" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"5"}]}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"unknown tool: '"$name"'"}}\n' "$id"
      fi
      ;;
    resources/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resources":[{"uri":"file:///fake/readme.txt","name":"Fake readme","description":"resource from the spawned process","mimeType":"text/plain","size":16,"annotations":{"audience":["assistant"],"priority":0.75}}]}}\n' "$id"
      ;;
    resources/read)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"contents":[{"uri":"file:///fake/readme.txt","mimeType":"text/plain","text":"spawned-resource"}]}}\n' "$id"
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"unsupported method"}}\n' "$id"
      ;;
  esac
done
"#;

/// Owns a temporary directory holding [`FAKE_MCP_SERVER_SH`], removing it on
/// drop.
pub struct FakeServerScript {
    dir: PathBuf,
    script: PathBuf,
}

impl FakeServerScript {
    pub fn path(&self) -> &Path {
        &self.script
    }
}

impl Drop for FakeServerScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Write the fake server to a unique temp directory and return a guard.
pub fn write_fake_mcp_server(label: &str) -> FakeServerScript {
    let dir = std::env::temp_dir().join(format!(
        "codewhale-mcp-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("fake MCP server dir");
    let script = dir.join("server.sh");
    std::fs::write(&script, FAKE_MCP_SERVER_SH).expect("write fake MCP server");
    FakeServerScript { dir, script }
}
