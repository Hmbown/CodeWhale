#!/usr/bin/env python3
"""Offline tests for scripts/catalog_models_dev.py (#4117)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "catalog_models_dev.py"
SEED = ROOT / "crates" / "config" / "assets" / "models_dev.bundled.json"


class CatalogModelsDevScriptTests(unittest.TestCase):
    def test_snapshot_check_validates_offline_seed(self) -> None:
        proc = subprocess.run(
            [sys.executable, str(SCRIPT), "snapshot", "--check", str(SEED)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("ok:", proc.stdout)
        self.assertIn("providers=", proc.stdout)

    def test_scrub_drops_api_key_fields(self) -> None:
        # Import helpers without network.
        sys.path.insert(0, str(ROOT / "scripts"))
        import catalog_models_dev as mod  # type: ignore

        dirty = {
            "models": {},
            "providers": {
                "deepseek": {
                    "api_key": "sk-should-never-persist",
                    "models": {"deepseek-v4-pro": {"id": "deepseek-v4-pro"}},
                }
            },
            "token": "nope",
        }
        clean = mod.scrub_secrets(dirty)
        self.assertNotIn("token", clean)
        self.assertNotIn("api_key", clean["providers"]["deepseek"])
        self.assertIn("models", clean["providers"]["deepseek"])

    def test_ensure_shape_rejects_empty_object(self) -> None:
        sys.path.insert(0, str(ROOT / "scripts"))
        import catalog_models_dev as mod  # type: ignore

        with self.assertRaises(SystemExit):
            mod.ensure_models_dev_shape({}, "test")

    def test_public_document_drops_api_key(self) -> None:
        sys.path.insert(0, str(ROOT / "scripts"))
        import catalog_models_dev as mod  # type: ignore

        dirty = {
            "models": {},
            "providers": {"deepseek": {"api_key": "sk-x", "models": {}}},
            "token": "nope",
        }
        clean = mod.public_models_dev_document(dirty)
        self.assertNotIn("token", clean)
        self.assertNotIn("api_key", clean["providers"]["deepseek"])

    def test_refresh_write_cache_is_rejected_without_writing(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            source = Path(td) / "catalog.json"
            target = Path(td) / "cache.json"
            source.write_text(
                json.dumps({"models": {}, "providers": {}, "api_key": "sk-nope"}),
                encoding="utf-8",
            )
            env = os.environ.copy()
            env["CODEWHALE_MODELS_DEV_PATH"] = str(source)

            proc = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "refresh",
                    "--write-cache",
                    str(target),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
                env=env,
            )

            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("disk writes are intentionally unsupported", proc.stderr)
            self.assertFalse(target.exists(), "refresh must remain dry-run only")


if __name__ == "__main__":
    unittest.main()
