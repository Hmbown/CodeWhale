import asyncio
import os
import pathlib
from loguru import logger
from dotenv import load_dotenv

load_dotenv()

DEEPSEEK_API_KEY   = os.getenv("DEEPSEEK_API_KEY", "")
DEEPSEEK_HTTP_PORT = int(os.getenv("DEEPSEEK_HTTP_PORT", "3000"))
MAX_RETRIES        = int(os.getenv("MAX_RETRIES", "5"))
RETRY_BASE_DELAY   = float(os.getenv("RETRY_BASE_DELAY", "2.0"))
DEV_MODE           = os.getenv("DEEPSEEK_DEV_MODE", "false").lower() == "true"

_HERE = pathlib.Path(__file__).resolve().parent       # .../core/
_env_dir = os.getenv("DEEPSEEK_TUI_DIR", "").strip()
TUI_DIR = pathlib.Path(_env_dir).resolve() if _env_dir else (_HERE.parent / "DeepSeek-TUI").resolve()
DEEPSEEK_BIN = os.getenv("DEEPSEEK_BIN", str(TUI_DIR / "target" / "release" / "deepseek.exe"))


class ProcessManager:
    def __init__(self):
        self._process = None
        self._supervising = False
        self._desired_running = False
        self._retry_count = 0
        self._manual_restart = False
        self._attached_pid: int | None = None
        self._restart_callbacks = []

    def on_restart(self, callback):
        self._restart_callbacks.append(callback)

    async def start(self):
        self._supervising = True
        self._desired_running = True
        self._retry_count = 0
        await self._launch_loop()

    async def shutdown(self):
        self._supervising = False
        self._desired_running = False
        if self._process and self.is_alive:
            try:
                self._process.terminate()
                await asyncio.wait_for(self._process.wait(), timeout=5.0)
            except (asyncio.TimeoutError, ProcessLookupError):
                self._process.kill()
        elif self._attached_pid is not None:
            await self._terminate_pid(self._attached_pid)
            self._attached_pid = None
        logger.info("DeepSeek-TUI supervisor stopped.")

    async def stop(self):
        self._desired_running = False
        if self._process and self.is_alive:
            try:
                self._process.terminate()
                await asyncio.wait_for(self._process.wait(), timeout=5.0)
            except (asyncio.TimeoutError, ProcessLookupError):
                self._process.kill()
        elif self._attached_pid is not None:
            await self._terminate_pid(self._attached_pid)
            self._attached_pid = None
        logger.info("DeepSeek-TUI runtime stopped.")

    async def ensure_started(self):
        self._desired_running = True
        self._manual_restart = False
        logger.info("DeepSeek-TUI runtime marked as desired-running.")

    async def restart(self):
        self._desired_running = True
        if not self.is_alive:
            if self._attached_pid is not None:
                await self._terminate_pid(self._attached_pid)
                self._attached_pid = None
            logger.info("Restart requested while app-server is stopped; launch loop will start it.")
            return

        if self._attached_pid is not None and (self._process is None or self._process.returncode is not None):
            await self._terminate_pid(self._attached_pid)
            self._attached_pid = None
            logger.info("DeepSeek-TUI attached runtime restart requested.")
            return

        self._manual_restart = True
        try:
            self._process.terminate()
            await asyncio.wait_for(self._process.wait(), timeout=5.0)
        except (asyncio.TimeoutError, ProcessLookupError):
            self._process.kill()
        logger.info("DeepSeek-TUI restart requested.")

    @property
    def is_alive(self):
        return (self._process is not None and self._process.returncode is None) or (
            self._attached_pid is not None and self._is_pid_running(self._attached_pid)
        )

    @property
    def pid(self):
        if self._process and self._process.returncode is None:
            return self._process.pid
        if self._attached_pid is not None and self._is_pid_running(self._attached_pid):
            return self._attached_pid
        return None

    @property
    def desired_running(self):
        return self._desired_running

    @property
    def supervising(self):
        return self._supervising

    @property
    def retry_count(self):
        return self._retry_count

    @property
    def attached_pid(self):
        return self._attached_pid

    @property
    def base_url(self):
        return f"http://localhost:{DEEPSEEK_HTTP_PORT}"

    def _build_cmd(self):
        env = {**os.environ, "DEEPSEEK_API_KEY": DEEPSEEK_API_KEY}
        if DEV_MODE:
            logger.info(f"DEV_MODE: cargo run in {TUI_DIR}")
            cmd = ["cargo", "run", "--bin", "deepseek", "--",
               "app-server", "--port", str(DEEPSEEK_HTTP_PORT)]
            kwargs = dict(cwd=str(TUI_DIR), env=env,
                          stdout=asyncio.subprocess.PIPE,
                          stderr=asyncio.subprocess.PIPE)
        else:
            logger.info(f"PROD_MODE: {DEEPSEEK_BIN}")
            cmd = [DEEPSEEK_BIN, "app-server", "--port", str(DEEPSEEK_HTTP_PORT)]
            kwargs = dict(env=env,
                          stdout=asyncio.subprocess.PIPE,
                          stderr=asyncio.subprocess.PIPE)
        return cmd, kwargs

    def _is_pid_running(self, pid: int) -> bool:
        try:
            os.kill(pid, 0)
            return True
        except PermissionError:
            return True
        except ProcessLookupError:
            return False
        except OSError:
            return False

    async def _probe_runtime_pid(self) -> int | None:
        command = (
            f"$conn = Get-NetTCPConnection -LocalPort {DEEPSEEK_HTTP_PORT} -State Listen "
            "-ErrorAction SilentlyContinue | Select-Object -First 1; "
            "if ($null -ne $conn) { $conn.OwningProcess }"
        )
        proc = await asyncio.create_subprocess_exec(
            "powershell",
            "-NoProfile",
            "-Command",
            command,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )
        stdout, _ = await proc.communicate()
        raw = stdout.decode(errors="replace").strip()
        if not raw:
            return None
        try:
            return int(raw.splitlines()[-1].strip())
        except ValueError:
            return None

    async def _terminate_pid(self, pid: int):
        logger.info(f"Terminating existing runtime on port {DEEPSEEK_HTTP_PORT} (pid={pid})")
        proc = await asyncio.create_subprocess_exec(
            "taskkill",
            "/PID",
            str(pid),
            "/T",
            "/F",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        await proc.communicate()

    async def _launch_loop(self):
        while self._supervising:
            if not self._desired_running:
                await asyncio.sleep(0.25)
                continue

            if self._attached_pid is not None and not self._is_pid_running(self._attached_pid):
                logger.info(f"Attached pid={self._attached_pid} is no longer running; clearing attachment.")
                self._attached_pid = None

            if self._attached_pid is None:
                existing_pid = await self._probe_runtime_pid()
                if existing_pid is not None:
                    self._attached_pid = existing_pid
                    logger.warning(
                        f"Port {DEEPSEEK_HTTP_PORT} is already in use by pid={existing_pid}; skipping duplicate launch."
                    )
                    await asyncio.sleep(2.0)
                    continue

            logger.info(f"Launching (attempt {self._retry_count + 1}/{MAX_RETRIES}) | TUI_DIR={TUI_DIR}")
            try:
                cmd, kwargs = self._build_cmd()
                self._process = await asyncio.create_subprocess_exec(*cmd, **kwargs)
                stdout, stderr = await self._process.communicate()
                code = self._process.returncode
                if not self._supervising:
                    break
                if not self._desired_running:
                    logger.info("Runtime exited after explicit stop request.")
                    self._process = None
                    self._manual_restart = False
                    continue
                if stderr:
                    error_text = stderr.decode(errors='replace')
                    if "os error 10048" in error_text or "Only one usage of each socket address" in error_text:
                        self._attached_pid = await self._probe_runtime_pid()
                        logger.warning(
                            f"Runtime port {DEEPSEEK_HTTP_PORT} is already occupied; attaching instead of retrying."
                        )
                        self._retry_count = 0
                        await asyncio.sleep(2.0)
                        continue
                logger.warning(f"Exited (code={code}), restarting...")
                if stderr:
                    logger.debug(f"stderr: {error_text[:500]}")
                if self._manual_restart:
                    self._retry_count = 0
                    self._manual_restart = False
                else:
                    self._retry_count += 1
                if self._retry_count >= MAX_RETRIES:
                    logger.error("Max retries reached.")
                    self._desired_running = False
                    break
                await asyncio.sleep(RETRY_BASE_DELAY ** self._retry_count)
                for cb in self._restart_callbacks:
                    asyncio.create_task(cb())
            except FileNotFoundError as e:
                logger.error(f"Not found: {e} | TUI_DIR={TUI_DIR} DEV_MODE={DEV_MODE}")
                self._desired_running = False
                break
            except PermissionError as e:
                logger.error(f"Permission denied: {e}")
                self._desired_running = False
                break
            except Exception as exc:
                logger.exception(f"Unexpected: {exc}")
                self._desired_running = False
                break
