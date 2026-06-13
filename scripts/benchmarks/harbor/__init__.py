"""
Harbor adapter for CodeWhale.

Lets Harbor evaluate CodeWhale as an agent on Terminal-Bench and other
Harbor-compatible datasets.

Usage (after pip install harbor):

    harbor run \\
      --dataset terminal-bench@2.0 \\
      --agent scripts.benchmarks.harbor.codewhale_agent:CodeWhaleAgent \\
      --model deepseek/deepseek-chat

Or register the agent name in Harbor's AgentName enum for shorter invocations.
"""

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


class CodeWhaleAgent(BaseInstalledAgent):
    """
    CodeWhale agent adapter for Harbor.

    Installs the ``codewhale`` CLI via npm into the task container and runs
    tasks in non-interactive exec mode with full tool access.
    """

    _OUTPUT_FILENAME = "codewhale.txt"

    CLI_FLAGS = [
        CliFlag(
            "max_subagents",
            cli="--max-subagents",
            type="int",
            default=4,
        ),
        CliFlag(
            "thinking",
            cli="--thinking",
            type="str",
            default="high",
        ),
        CliFlag(
            "provider",
            cli="--provider",
            type="str",
            default=None,
        ),
    ]

    @staticmethod
    def name() -> str:
        return "codewhale"

    def version(self) -> str | None:
        return getattr(self, "_version", None)

    def get_version_command(self) -> str | None:
        return "codewhale --version 2>/dev/null || codewhale-tui --version 2>/dev/null"

    def parse_version(self, stdout: str) -> str:
        text = stdout.strip()
        for line in text.splitlines():
            line = line.strip()
            if line:
                # Strip any prefix like "codewhale " or "codewhale-cli "
                for prefix in ("codewhale-tui ", "codewhale-cli ", "codewhale "):
                    if line.lower().startswith(prefix):
                        return line[len(prefix):]
                return line
        return text

    async def install(self, environment: BaseEnvironment) -> None:
        """Install CodeWhale via npm in the container."""
        # Install system dependencies
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

        # Install Node.js if not present (some images lack it)
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

        # Install CodeWhale CLI via npm
        await self.exec_as_agent(
            environment,
            command="npm install -g codewhale",
        )

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        """Run CodeWhale in non-interactive exec mode on the task."""
        escaped_instruction = shlex.quote(instruction)

        # Build CLI flags from agent config
        cli_flags = self.build_cli_flags()
        extra_flags = (cli_flags + " ") if cli_flags else ""

        # Determine API key environment variables to forward
        env: dict[str, str] = {}

        # DeepSeek
        deepseek_key = os.environ.get("DEEPSEEK_API_KEY", "")
        if deepseek_key:
            env["DEEPSEEK_API_KEY"] = deepseek_key

        # OpenRouter (fallback)
        openrouter_key = os.environ.get("OPENROUTER_API_KEY", "")
        if openrouter_key:
            env["OPENROUTER_API_KEY"] = openrouter_key

        # Generic OpenAI-compatible
        openai_key = os.environ.get("OPENAI_API_KEY", "")
        if openai_key:
            env["OPENAI_API_KEY"] = openai_key

        # Build model flag if model_name is provided
        model_flag = ""
        if self.model_name:
            # Harbor passes model as "provider/model"; CodeWhale uses --model
            model_flag = f"--model {shlex.quote(self.model_name)} "

        output_path = f"/logs/agent/{self._OUTPUT_FILENAME}"

        # Run CodeWhale in non-interactive YOLO exec mode
        # --yolo enables full tool access (auto-approved)
        # --auto runs non-interactively and exits when done
        # --stream-json gives us structured output for trajectory parsing
        await self.exec_as_agent(
            environment,
            command=(
                f"codewhale exec --yolo --auto --stream-json "
                f"{model_flag}{extra_flags}"
                f"--workspace /workspace "
                f"{escaped_instruction} "
                f"2>&1 | tee {shlex.quote(output_path)}"
            ),
            env=env if env else None,
        )

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Parse CodeWhale's output for any post-run metadata."""
        # CodeWhale writes its results to the working tree as git diffs.
        # Harbor's eval harness inspects the workspace directly, so no
        # special trajectory parsing is needed for basic eval.
        pass
