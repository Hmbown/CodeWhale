"""Harbor adapter for Codex CLI."""

import json
import os
import shlex
from pathlib import Path, PurePosixPath
from typing import Any

from harbor.agents.installed.base import (
    BaseInstalledAgent,
    CliFlag,
    with_prompt_template,
)
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


class CodexAgent(BaseInstalledAgent):
    """Codex CLI agent adapter for Harbor."""

    _OUTPUT_FILENAME = "codex.txt"

    CLI_FLAGS = [
        CliFlag(
            "allowed-tools",
            cli="--allowed-tools",
            type="str",
            default="Bash,Read,Write,Edit,Glob,Grep",
        ),
    ]

    @staticmethod
    def name() -> str:
        return "codex"

    def version(self) -> str | None:
        return getattr(self, "_version", None)

    def get_version_command(self) -> str | None:
        return "codex --version 2>/dev/null || codex-cli --version 2>/dev/null"

    def parse_version(self, stdout: str) -> str:
        text = stdout.strip()
        for line in text.splitlines():
            line = line.strip()
            if line:
                for prefix in ("codex-cli ", "codex "):
                    if line.lower().startswith(prefix):
                        return line[len(prefix):]
                return line
        return text

    async def install(self, environment: BaseEnvironment) -> None:
        """Install Codex CLI in the container."""
        await self.exec_as_root(
            environment,
            command=(
                "if ldd --version 2>&1 | grep -qi musl || [ -f /etc/alpine-release ]; then"
                "  apk add --no-cache curl bash nodejs npm git ripgrep;"
                " elif command -v apt-get &>/dev/null; then"
                "  apt-get update && apt-get install -y curl git ripgrep;"
                " elif command -v yum &>/dev/null; then"
                "  yum install -y curl git ripgrep;"
                " fi"
            ),
            env={"DEBIAN_FRONTEND": "noninteractive"},
        )

        await self.exec_as_root(
            environment,
            command=(
                "if ! command -v node &>/dev/null; then"
                "  curl -fsSL https://deb.nodesource.com/setup_20.x | bash - &&"
                "  apt-get install -y nodejs;"
                " fi"
            ),
            env={"DEBIAN_FRONTEND": "noninteractive"},
        )

        await self.exec_as_agent(
            environment,
            command="npm install -g codex",
        )

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        """Run Codex CLI in non-interactive exec mode."""
        escaped_instruction = shlex.quote(instruction)

        cli_flags = self.build_cli_flags()
        extra_flags = (cli_flags + " ") if cli_flags else ""

        model_flag = ""
        if self.model_name:
            model_flag = f"--model {shlex.quote(self.model_name)} "

        # Forward API keys
        env: dict[str, str] = {}
        for key in ("CODEX_API_KEY", "DEEPSEEK_API_KEY", "OPENAI_API_KEY",
                     "ANTHROPIC_API_KEY", "OPENROUTER_API_KEY"):
            val = os.environ.get(key, "")
            if val:
                env[key] = val

        output_path = f"/logs/agent/{self._OUTPUT_FILENAME}"

        await self.exec_as_agent(
            environment,
            command=(
                f"codex exec --yes "
                f"{model_flag}{extra_flags}"
                f"--workspace /workspace "
                f"{escaped_instruction} "
                f"2>&1 | tee {shlex.quote(output_path)}"
                f" || true"
            ),
            env=env if env else None,
        )

    def populate_context_post_run(self, context: AgentContext) -> None:
        pass
