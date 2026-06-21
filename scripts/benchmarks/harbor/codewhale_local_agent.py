"""Harbor adapter that runs a local CodeWhale Linux binary artifact.

The stock CodeWhale Harbor adapter installs from npm, but npm may lag the local
release branch. This adapter uploads explicit Linux binaries into each
Terminal-Bench task container so benchmark rows identify the intended local
build.
"""

from __future__ import annotations

import os
import shlex
from pathlib import Path, PurePosixPath

from harbor.agents.installed.base import BaseInstalledAgent, CliFlag, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.models.trial.paths import EnvironmentPaths

CODEWHALE_LINUX_BIN_ENV = "CODEWHALE_LINUX_BIN"
CODEWHALE_TUI_LINUX_BIN_ENV = "CODEWHALE_TUI_LINUX_BIN"
HARNESS_LIBRARY = "/usr/local/lib/codewhale-bench-harness.sh"
HARNESS_TIMEOUTS = {
    "default_command_s": 30,
    "build_command_s": 300,
    "background_start_s": 600,
    "readiness_probe_s": 120,
    "verifier_s": 900,
}
TASK_READINESS_PROBES = {
    "configure-git-webserver": (
        "curl -fsS http://127.0.0.1:8080/ >/dev/null && "
        "rm -rf /tmp/codewhale-readiness-git-probe && "
        "git clone http://127.0.0.1:8080/repo.git /tmp/codewhale-readiness-git-probe"
    ),
    "qemu-alpine-ssh": (
        "timeout 20 bash -lc 'printf \"\\n\" | nc -w 5 127.0.0.1 6665 | "
        "grep -Ei \"login:|localhost login\"'"
    ),
    "qemu-startup": (
        "timeout 20 bash -lc 'printf \"\\n\" | nc -w 5 127.0.0.1 6665 | "
        "grep -Ei \"login:|localhost login\"'"
    ),
}


HARNESS_LIBRARY_BODY = r"""#!/usr/bin/env bash
# Shell helpers exposed to benchmark agents. They keep background service
# lifecycle and readiness probes consistent across Terminal-Bench tasks.

codewhale_background_root() {
  local root="${CODEWHALE_BACKGROUND_ROOT:-/tmp/codewhale-background}"
  mkdir -p "$root"
  printf '%s\n' "$root"
}

start_background() {
  local command="$1"
  local name="$2"
  local ready_probe="${3:-}"
  local timeout_s="${4:-600}"
  local root log pid_file pid
  root="$(codewhale_background_root)"
  log="$root/$name.log"
  pid_file="$root/$name.pid"
  if [[ -s "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
    printf 'background_already_running name=%s pid=%s log=%s\n' "$name" "$(cat "$pid_file")" "$log"
  else
    rm -f "$log"
    setsid bash -lc "$command" >"$log" 2>&1 < /dev/null &
    pid="$!"
    printf '%s\n' "$pid" >"$pid_file"
    printf 'background_started name=%s pid=%s log=%s\n' "$name" "$pid" "$log"
  fi
  if [[ -n "$ready_probe" ]]; then
    assert_ready "$name" "$ready_probe" "$timeout_s"
  fi
}

read_background_log() {
  local name="$1"
  local since="${2:-200}"
  local root log
  root="$(codewhale_background_root)"
  log="$root/$name.log"
  if [[ ! -f "$log" ]]; then
    printf 'background_log_missing name=%s log=%s\n' "$name" "$log" >&2
    return 1
  fi
  tail -n "$since" "$log"
}

stop_background() {
  local name="$1"
  local root pid_file pid
  root="$(codewhale_background_root)"
  pid_file="$root/$name.pid"
  if [[ ! -s "$pid_file" ]]; then
    printf 'background_not_running name=%s\n' "$name"
    return 0
  fi
  pid="$(cat "$pid_file")"
  if kill -0 "$pid" 2>/dev/null; then
    kill "-$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
    sleep 1
    kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true
  fi
  rm -f "$pid_file"
  printf 'background_stopped name=%s pid=%s\n' "$name" "$pid"
}

assert_ready() {
  local name="$1"
  local ready_probe="$2"
  local timeout_s="${3:-120}"
  local deadline=$((SECONDS + timeout_s))
  until bash -lc "$ready_probe"; do
    if (( SECONDS >= deadline )); then
      printf 'background_not_ready name=%s timeout_s=%s probe=%s\n' "$name" "$timeout_s" "$ready_probe" >&2
      read_background_log "$name" 120 >&2 || true
      return 124
    fi
    sleep 2
  done
  printf 'background_ready name=%s probe=%s\n' "$name" "$ready_probe"
}
"""


class CodeWhaleLocalAgent(BaseInstalledAgent):
    """Run CodeWhale from host-built Linux binaries inside a Harbor task."""

    _OUTPUT_FILENAME = "codewhale.txt"
    _REMOTE_BIN = "/usr/local/bin/codewhale"
    _REMOTE_TUI_BIN = "/usr/local/bin/codewhale-tui"

    CLI_FLAGS = [
        CliFlag("max_subagents", cli="--max-subagents", type="int", default=None),
    ]

    def __init__(
        self,
        *args,
        local_binary_path: str | None = None,
        local_tui_binary_path: str | None = None,
        provider: str | None = None,
        reasoning_effort: str | None = None,
        **kwargs,
    ):
        super().__init__(*args, **kwargs)
        self._local_binary_path = self._resolve_local_path(
            local_binary_path,
            CODEWHALE_LINUX_BIN_ENV,
        )
        self._local_tui_binary_path = self._resolve_local_path(
            local_tui_binary_path,
            CODEWHALE_TUI_LINUX_BIN_ENV,
        )
        self._provider_override = provider
        self._reasoning_effort = self._normalize_reasoning_effort(reasoning_effort)

    @staticmethod
    def _resolve_local_path(explicit: str | None, env_key: str) -> Path | None:
        value = explicit or os.environ.get(env_key)
        if value and value.strip():
            return Path(value.strip()).expanduser()
        return None

    @staticmethod
    def name() -> str:
        return "codewhale-local"

    def get_version_command(self) -> str | None:
        return f"{self._REMOTE_BIN} --version"

    def parse_version(self, stdout: str) -> str:
        text = stdout.strip()
        for line in text.splitlines():
            line = line.strip()
            if line:
                for prefix in ("codewhale-tui ", "codewhale-cli ", "codewhale "):
                    if line.lower().startswith(prefix):
                        return line[len(prefix) :]
                return line
        return text

    async def install(self, environment: BaseEnvironment) -> None:
        if self._local_binary_path is None:
            raise FileNotFoundError(
                "CodeWhale Linux binary path is required; pass "
                "local_binary_path=... or set CODEWHALE_LINUX_BIN."
            )
        if self._local_tui_binary_path is None:
            raise FileNotFoundError(
                "CodeWhale TUI Linux binary path is required; pass "
                "local_tui_binary_path=... or set CODEWHALE_TUI_LINUX_BIN."
            )
        if not self._local_binary_path.is_file():
            raise FileNotFoundError(f"CodeWhale Linux binary not found: {self._local_binary_path}")
        if not self._local_tui_binary_path.is_file():
            raise FileNotFoundError(
                f"CodeWhale TUI Linux binary not found: {self._local_tui_binary_path}"
            )

        await self.exec_as_root(
            environment,
            command=(
                "if command -v apt-get >/dev/null 2>&1; then "
                "apt-get update && "
                "ssl_pkg=''; "
                "if apt-cache show libssl3 >/dev/null 2>&1; then ssl_pkg=libssl3; "
                "elif apt-cache show libssl1.1 >/dev/null 2>&1; then ssl_pkg=libssl1.1; fi; "
                "DEBIAN_FRONTEND=noninteractive apt-get install -y "
                "--no-install-recommends bash ca-certificates git ripgrep libdbus-1-3 $ssl_pkg; "
                "elif command -v apk >/dev/null 2>&1; then "
                "apk add --no-cache bash ca-certificates git ripgrep openssl dbus-libs; "
                "fi"
            ),
        )
        await environment.upload_file(self._local_binary_path, self._REMOTE_BIN)
        await environment.upload_file(self._local_tui_binary_path, self._REMOTE_TUI_BIN)
        await self._install_harness_library(environment)
        await self.exec_as_root(
            environment,
            command=(
                f"chmod 755 {self._REMOTE_BIN} {self._REMOTE_TUI_BIN} && "
                f"ln -sf {self._REMOTE_BIN} /usr/local/bin/codew && "
                f"{self._REMOTE_BIN} --version && {self._REMOTE_TUI_BIN} --version"
            ),
        )
        await self._run_artifact_preflight(environment)

    async def _install_harness_library(self, environment: BaseEnvironment) -> None:
        quoted_body = shlex.quote(HARNESS_LIBRARY_BODY)
        await self.exec_as_root(
            environment,
            command=(
                "mkdir -p /usr/local/lib && "
                f"printf %s {quoted_body} > {shlex.quote(HARNESS_LIBRARY)} && "
                f"chmod 644 {shlex.quote(HARNESS_LIBRARY)}"
            ),
        )

    async def _run_artifact_preflight(self, environment: BaseEnvironment) -> None:
        agent_dir = shlex.quote(EnvironmentPaths.agent_dir.as_posix())
        preflight_path = shlex.quote(
            PurePosixPath(EnvironmentPaths.agent_dir / "codewhale-artifact-preflight.txt").as_posix()
        )
        await self.exec_as_root(
            environment,
            command=(
                f"mkdir -p {agent_dir}; "
                "set +e; "
                "{ "
                "echo '$ codewhale --version'; "
                f"{self._REMOTE_BIN} --version; version_status=$?; "
                "echo '$ ldd \"$(command -v codewhale)\"'; "
                "ldd \"$(command -v codewhale)\" || true; "
                "echo '$ /lib/x86_64-linux-gnu/libc.so.6 || true'; "
                "/lib/x86_64-linux-gnu/libc.so.6 || true; "
                "exit $version_status; "
                f"}} > {preflight_path} 2>&1; "
                "status=$?; "
                f"cat {preflight_path}; "
                "if [ $status -ne 0 ] || "
                f"grep -Eiq 'error while loading shared libraries|GLIBC_[0-9]|version .* not found|libssl[^[:space:]]*.*not found|libcrypto[^[:space:]]*.*not found|libdbus[^[:space:]]*.*not found|OpenSSL.*(not found|incompatible)' {preflight_path}; "
                "then "
                "echo 'artifact_incompatible: CodeWhale Linux artifact failed container preflight' >&2; "
                "exit 86; "
                "fi"
            ),
        )

    def _provider_and_model(self) -> tuple[str, str]:
        raw = self.model_name or "deepseek/deepseek-v4-flash"
        if "/" in raw:
            provider, model = raw.split("/", 1)
        else:
            provider, model = "deepseek", raw
        if self._provider_override:
            provider = self._provider_override
        if provider == "openai-compatible":
            provider = "openai"
        return provider, model

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

    @staticmethod
    def _context_task_name(context: AgentContext) -> str | None:
        for attr in ("task_name", "name", "id"):
            value = getattr(context, attr, None)
            if isinstance(value, str) and value.strip():
                return value.strip()
        task = getattr(context, "task", None)
        if task is not None:
            for attr in ("name", "task_name", "id"):
                value = getattr(task, attr, None)
                if isinstance(value, str) and value.strip():
                    return value.strip()
        return None

    @staticmethod
    def _readiness_probe_for_task(task_name: str | None) -> str | None:
        if not task_name:
            return None
        normalized = task_name.strip().lower()
        for key, probe in TASK_READINESS_PROBES.items():
            if key in normalized:
                return probe
        return None

    async def _detect_verifier_surfaces(
        self,
        environment: BaseEnvironment,
        env: dict[str, str],
        workspace: str,
    ) -> list[str]:
        result = await self.exec_as_agent(
            environment,
            command=(
                "set +e; "
                "for path in /tests ./tests ./tests/verify.sh task.yaml pytest.ini pyproject.toml setup.cfg tox.ini README.md README.rst README.txt; do "
                "[ -e \"$path\" ] && printf '%s\\n' \"$path\"; "
                "done; "
                "find . -maxdepth 2 -type f \\( -name 'test_*.py' -o -name '*_test.py' -o -name 'Makefile' \\) -print 2>/dev/null | head -n 12"
            ),
            env=env,
            cwd=workspace,
        )
        seen: set[str] = set()
        surfaces: list[str] = []
        for line in (result.stdout or "").splitlines():
            item = line.strip()
            if item and item not in seen:
                surfaces.append(item)
                seen.add(item)
        return surfaces[:16]

    @staticmethod
    def _harness_note(
        verifier_surfaces: list[str],
        task_name: str | None,
        readiness_probe: str | None,
    ) -> str:
        lines = [
            "Benchmark harness note:",
            f"- Background service helpers are available with: source {HARNESS_LIBRARY}",
            "- Helpers: start_background COMMAND NAME READY_PROBE TIMEOUT_S; read_background_log NAME [LINES]; stop_background NAME; assert_ready NAME READY_PROBE TIMEOUT_S.",
            "- Timeout classes: default commands 30s, build commands 300s, background starts 600s, readiness probes 120s, verifiers 900s.",
        ]
        if task_name:
            lines.append(f"- Task name: {task_name}")
        if readiness_probe:
            lines.append(f"- Task readiness probe: {readiness_probe}")
        if verifier_surfaces:
            lines.append("- Detected verifier/test surfaces:")
            lines.extend(f"  - {surface}" for surface in verifier_surfaces)
        else:
            lines.append("- Detected verifier/test surfaces: none from the standard quick scan.")
        return "\n".join(lines)

    @staticmethod
    def _key_env_for_provider(provider: str) -> str:
        return {
            "deepseek": "DEEPSEEK_API_KEY",
            "openrouter": "OPENROUTER_API_KEY",
            "openai": "OPENAI_API_KEY",
            "zai": "ZAI_API_KEY",
            "z-ai": "ZAI_API_KEY",
        }.get(provider, f"{provider.replace('-', '_').upper()}_API_KEY")

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        provider, model = self._provider_and_model()
        key_env = self._key_env_for_provider(provider)
        api_key = self._get_env(key_env)
        if not api_key:
            raise ValueError(f"{key_env} is required for CodeWhale {provider} runs")

        pwd = await self.exec_as_agent(environment, "pwd")
        workspace = (pwd.stdout or "/workspace").strip() or "/workspace"
        task_name = self._context_task_name(context)
        readiness_probe = self._readiness_probe_for_task(task_name)
        output_path = PurePosixPath(EnvironmentPaths.agent_dir / self._OUTPUT_FILENAME)
        harness_note_path = PurePosixPath(EnvironmentPaths.agent_dir / "codewhale-harness-note.txt")
        cli_flags = self.build_cli_flags()
        extra_flags = f"{cli_flags} " if cli_flags else ""
        config_path = PurePosixPath("/tmp/codewhale-home/config.toml")
        config_arg = (
            f"--config {shlex.quote(config_path.as_posix())} "
            if self._reasoning_effort
            else ""
        )

        env: dict[str, str] = {
            key_env: api_key,
            "AWS_LC_SYS_NO_ASM": "1",
            "CODEWHALE_HOME": "/tmp/codewhale-home",
            "CODEWHALE_PROVIDER": provider,
            "CODEWHALE_MODEL": model,
        }
        for name in ("DEEPSEEK_BASE_URL", "CODEWHALE_BASE_URL", "OPENROUTER_BASE_URL"):
            value = self._get_env(name)
            if value:
                env[name] = value

        verifier_surfaces = await self._detect_verifier_surfaces(environment, env, workspace)
        harness_note = self._harness_note(verifier_surfaces, task_name, readiness_probe)

        escaped_instruction = shlex.quote(f"{harness_note}\n\n{instruction}")
        config_lines = [
            f'provider = "{provider}"',
            f'default_text_model = "{model}"',
        ]
        if self._reasoning_effort:
            config_lines.append(f'reasoning_effort = "{self._reasoning_effort}"')
        write_config = "printf '%s\\n' " + " ".join(
            shlex.quote(line) for line in config_lines
        ) + f" > {shlex.quote(config_path.as_posix())}"
        await self.exec_as_agent(
            environment,
            command=(
                f"mkdir -p {shlex.quote(EnvironmentPaths.agent_dir.as_posix())} "
                '"/tmp/codewhale-home" && '
                f"{write_config} && "
                f"printf '%s\\n' {shlex.quote(harness_note)} > {shlex.quote(harness_note_path.as_posix())}"
            ),
            env=env,
            cwd=workspace,
        )
        await self.exec_as_agent(
            environment,
            command=(
                "set +e; "
                f"{self._REMOTE_BIN} "
                f"{config_arg}"
                f"--provider {shlex.quote(provider)} "
                f"--model {shlex.quote(model)} "
                f"--workspace {shlex.quote(workspace)} "
                "--yolo "
                "exec --auto --output-format stream-json "
                f"{extra_flags}"
                f"-- {escaped_instruction} "
                f"2>&1 </dev/null | tee {shlex.quote(output_path.as_posix())}; "
                "status=${PIPESTATUS[0]}; "
                "rm -rf .codewhale .deepseek abs /tmp/codewhale-home; "
                "exit $status"
            ),
            env=env,
            cwd=workspace,
        )

    def populate_context_post_run(self, context: AgentContext) -> None:
        task_name = self._context_task_name(context)
        metadata = {
            "task_name": task_name,
            "readiness_probe": self._readiness_probe_for_task(task_name),
            "harness_timeouts": HARNESS_TIMEOUTS,
            "harness_note_path": str(self.logs_dir / "codewhale-harness-note.txt"),
        }
        output_path = self.logs_dir / self._OUTPUT_FILENAME
        if output_path.exists():
            metadata["codewhale_log"] = str(output_path)
        metadata["reasoning_effort"] = self._reasoning_effort
        context.metadata = metadata
