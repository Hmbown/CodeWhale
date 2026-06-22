"""Thin Harbor agent that calls DeepSeek directly with shell/file tools.

This is a deliberately small baseline for CodeWhale-vs-API comparisons. It
does not install an agent in the task container; the Harbor adapter calls
DeepSeek's OpenAI-compatible chat-completions endpoint from the host and uses
Harbor environment operations for the only two exposed tools.
"""

from __future__ import annotations

import asyncio
import base64
import json
import os
import shlex
import urllib.error
import urllib.request
from pathlib import PurePosixPath
from typing import Any

from harbor.agents.base import BaseAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


class DeepSeekDirectAgent(BaseAgent):
    """Direct DeepSeek API baseline with a minimal tool loop."""

    _OUTPUT_FILENAME = "direct-deepseek.jsonl"

    def __init__(
        self,
        *args: Any,
        reasoning_effort: str | None = None,
        max_steps: int = 24,
        max_tokens: int = 4096,
        base_url: str | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(*args, **kwargs)
        self._reasoning_effort = self._normalize_reasoning_effort(reasoning_effort)
        self._max_steps = int(max_steps)
        self._max_tokens = int(max_tokens)
        self._base_url = (
            base_url
            or os.environ.get("DEEPSEEK_BASE_URL")
            or os.environ.get("CODEWHALE_BASE_URL")
            or "https://api.deepseek.com/beta"
        ).rstrip("/")
        self._input_tokens = 0
        self._output_tokens = 0
        self._cache_tokens = 0
        self._reasoning_tokens = 0

    @staticmethod
    def name() -> str:
        return "deepseek-direct"

    def version(self) -> str | None:
        return "direct-chat-completions"

    async def setup(self, environment: BaseEnvironment) -> None:
        return None

    @staticmethod
    def _normalize_reasoning_effort(reasoning_effort: str | None) -> str | None:
        if reasoning_effort is None:
            return None
        normalized = reasoning_effort.strip().lower()
        aliases = {
            "none": "off",
            "disabled": "off",
            "false": "off",
            "medium": "high",
            "mid": "high",
            "maximum": "max",
            "xhigh": "max",
            "ultracode": "max",
        }
        normalized = aliases.get(normalized, normalized)
        if normalized not in {"off", "high", "max"}:
            raise ValueError(
                "reasoning_effort must be one of off, high, or max "
                f"(got {reasoning_effort!r})"
            )
        return normalized

    def _provider_and_model(self) -> tuple[str, str]:
        raw = self.model_name or "deepseek/deepseek-v4-flash"
        if "/" in raw:
            provider, model = raw.split("/", 1)
        else:
            provider, model = "deepseek", raw
        return provider, model

    @staticmethod
    def _tools() -> list[dict[str, Any]]:
        return [
            {
                "type": "function",
                "function": {
                    "name": "exec_shell",
                    "description": "Run a shell command in the task workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {"type": "string"},
                            "timeout_sec": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 600,
                            },
                        },
                        "required": ["command"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Write UTF-8 text to a file in the task container.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "content": {"type": "string"},
                        },
                        "required": ["path", "content"],
                    },
                },
            },
        ]

    def _payload(self, messages: list[dict[str, Any]], require_tool: bool = False) -> dict[str, Any]:
        _, model = self._provider_and_model()
        payload: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "tools": self._tools(),
            "temperature": 0,
            "max_tokens": self._max_tokens,
            "stream": False,
        }
        if self._reasoning_effort == "off":
            payload["tool_choice"] = "required" if require_tool else "auto"
            payload["thinking"] = {"type": "disabled"}
        elif self._reasoning_effort:
            # DeepSeek thinking mode rejects explicit tool_choice, including
            # "required"; omit it and let the model choose from the tool list.
            payload["reasoning_effort"] = self._reasoning_effort
            payload["thinking"] = {"type": "enabled"}
        else:
            payload["tool_choice"] = "required" if require_tool else "auto"
        return payload

    def _api_key(self) -> str:
        key = os.environ.get("DEEPSEEK_API_KEY")
        if not key:
            raise ValueError("DEEPSEEK_API_KEY is required")
        return key

    async def _call_deepseek(
        self, messages: list[dict[str, Any]], require_tool: bool = False
    ) -> dict[str, Any]:
        payload = self._payload(messages, require_tool=require_tool)

        def post() -> dict[str, Any]:
            request = urllib.request.Request(
                f"{self._base_url}/chat/completions",
                data=json.dumps(payload).encode("utf-8"),
                headers={
                    "Authorization": f"Bearer {self._api_key()}",
                    "Content-Type": "application/json",
                },
                method="POST",
            )
            try:
                with urllib.request.urlopen(request, timeout=300) as response:
                    return json.loads(response.read().decode("utf-8"))
            except urllib.error.HTTPError as exc:
                body = exc.read().decode("utf-8", errors="replace")
                raise RuntimeError(f"DeepSeek HTTP {exc.code}: {body}") from exc

        return await asyncio.to_thread(post)

    def _record_usage(self, response: dict[str, Any]) -> None:
        usage = response.get("usage")
        if not isinstance(usage, dict):
            return
        self._input_tokens += int(usage.get("prompt_tokens") or usage.get("input_tokens") or 0)
        self._output_tokens += int(
            usage.get("completion_tokens") or usage.get("output_tokens") or 0
        )
        prompt_details = usage.get("prompt_tokens_details")
        if isinstance(prompt_details, dict):
            self._cache_tokens += int(prompt_details.get("cached_tokens") or 0)
        completion_details = usage.get("completion_tokens_details")
        if isinstance(completion_details, dict):
            self._reasoning_tokens += int(completion_details.get("reasoning_tokens") or 0)

    def _log(self, obj: dict[str, Any]) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        with (self.logs_dir / self._OUTPUT_FILENAME).open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(obj, ensure_ascii=False, sort_keys=True) + "\n")

    @staticmethod
    def _compact_exec_result(stdout: str | None, stderr: str | None, code: int) -> str:
        out = stdout or ""
        err = stderr or ""
        text = f"exit_code={code}\nstdout:\n{out}\nstderr:\n{err}"
        if len(text) > 12000:
            return text[:12000] + "\n...[truncated]"
        return text

    async def _run_tool(
        self,
        tool_name: str,
        arguments: dict[str, Any],
        environment: BaseEnvironment,
        workspace: str,
    ) -> str:
        if tool_name == "exec_shell":
            command = str(arguments.get("command") or "")
            timeout_sec = int(arguments.get("timeout_sec") or 120)
            timeout_sec = max(1, min(timeout_sec, 600))
            result = await environment.exec(
                command,
                cwd=workspace,
                timeout_sec=timeout_sec,
            )
            return self._compact_exec_result(result.stdout, result.stderr, result.return_code)

        if tool_name == "write_file":
            path = str(arguments.get("path") or "")
            content = str(arguments.get("content") or "")
            if not path:
                return "error: missing path"
            encoded = base64.b64encode(content.encode("utf-8")).decode("ascii")
            parent = PurePosixPath(path).parent.as_posix()
            command = (
                f"mkdir -p {shlex.quote(parent)} && "
                f"printf %s {shlex.quote(encoded)} | base64 -d > {shlex.quote(path)}"
            )
            result = await environment.exec(command, cwd=workspace, timeout_sec=60)
            return self._compact_exec_result(result.stdout, result.stderr, result.return_code)

        return f"error: unknown tool {tool_name}"

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        pwd = await environment.exec("pwd", timeout_sec=10)
        workspace = (pwd.stdout or "/app").strip() or "/app"
        system = (
            "You are a terminal coding agent inside a benchmark container. "
            "Use the provided tools to inspect files, run commands, and write the required artifacts. "
            "The benchmark only grades files and container state, not prose. "
            "Do not answer with an explanation when a file must be saved. "
            "If the task asks to save a file, call write_file with the exact requested path. "
            "Complete the task directly; when the required file or state is done, reply with DONE."
        )
        messages: list[dict[str, Any]] = [
            {"role": "system", "content": system},
            {"role": "user", "content": instruction},
        ]

        for step in range(self._max_steps):
            require_tool = step == 0 or (
                messages[-1].get("role") == "user"
                and "did not call a tool" in str(messages[-1].get("content", ""))
            )
            response = await self._call_deepseek(messages, require_tool=require_tool)
            self._record_usage(response)
            self._log({"type": "response", "step": step, "response": response})
            choice = (response.get("choices") or [{}])[0]
            message = choice.get("message") or {}
            tool_calls = message.get("tool_calls") or []
            messages.append(message)
            if not tool_calls:
                if "DONE" in str(message.get("content") or "").upper():
                    break
                if step < self._max_steps - 1:
                    messages.append(
                        {
                            "role": "user",
                            "content": (
                                "You did not call a tool. This benchmark will fail unless "
                                "you create the required artifact in the container. Use "
                                "write_file or exec_shell now; do not continue in prose."
                            ),
                        }
                    )
                    continue
                break
            for tool_call in tool_calls:
                function = tool_call.get("function") or {}
                tool_name = function.get("name") or ""
                raw_args = function.get("arguments") or "{}"
                try:
                    arguments = json.loads(raw_args) if isinstance(raw_args, str) else raw_args
                except json.JSONDecodeError:
                    arguments = {"command": str(raw_args)}
                if not isinstance(arguments, dict):
                    arguments = {}
                output = await self._run_tool(tool_name, arguments, environment, workspace)
                self._log(
                    {
                        "type": "tool_result",
                        "step": step,
                        "tool_call_id": tool_call.get("id"),
                        "tool_name": tool_name,
                        "arguments": arguments,
                        "output": output,
                    }
                )
                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": tool_call.get("id"),
                        "content": output,
                    }
                )

        context.n_input_tokens = self._input_tokens
        context.n_output_tokens = self._output_tokens
        context.n_cache_tokens = self._cache_tokens
        context.metadata = {
            "direct_deepseek_log": str(self.logs_dir / self._OUTPUT_FILENAME),
            "reasoning_effort": self._reasoning_effort,
            "reasoning_tokens": self._reasoning_tokens,
        }
