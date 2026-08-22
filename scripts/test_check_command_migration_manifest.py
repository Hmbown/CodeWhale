#!/usr/bin/env python3
"""Hermetic tests for the FEAT-015 command migration manifest gate.

Covers the Deep-Dive schema and frontier rules:

- valid root frontier; valid parent-to-all-children replacement; valid removal
- rejected partial split; rejected arbitrary addition
- duplicate/unsorted entry; unknown schema/tag/field; unsupported syntax
- missing/overlapping selector; const normalization
- integer/character/byte/path const atoms; out-of-range byte diagnostics
"""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "check-command-migration-manifest.py"
SPEC = importlib.util.spec_from_file_location("command_migration", SCRIPT)
assert SPEC and SPEC.loader
mod = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = mod
SPEC.loader.exec_module(mod)


def sample_topology() -> dict:
    """A minimal two-group topology with one documented slice per group."""
    return {
        "schema_version": 1,
        "topology": {
            "utility": {
                "kind": "group",
                "scope": ["crates/tui/src/commands/groups/utility/mod.rs"],
                "slices": [],
            },
            "session": {
                "kind": "group",
                "scope": ["crates/tui/src/commands/groups/session/mod.rs"],
                "slices": [
                    {
                        "name": "session::lifecycle",
                        "kind": "slice",
                        "scope": ["crates/tui/src/commands/groups/session/branch.rs"],
                    },
                    {
                        "name": "session::control",
                        "kind": "slice",
                        "scope": ["crates/tui/src/commands/groups/session/relay.rs"],
                    },
                ],
            },
        },
        "frontier": ["session", "utility"],
    }


class SchemaTests(unittest.TestCase):
    def test_valid_document_passes(self) -> None:
        self.assertEqual(mod.validate_topology_document(sample_topology()), [])

    def test_unknown_schema_version_fails(self) -> None:
        doc = sample_topology()
        doc["schema_version"] = 2
        violations = mod.validate_topology_document(doc)
        self.assertEqual(len(violations), 1)
        self.assertIn("schema_version", str(violations[0]))

    def test_missing_schema_version_fails(self) -> None:
        doc = sample_topology()
        del doc["schema_version"]
        self.assertTrue(mod.validate_topology_document(doc))

    def test_unknown_group_field_fails(self) -> None:
        doc = sample_topology()
        doc["topology"]["utility"]["extra"] = True
        violations = mod.validate_topology_document(doc)
        self.assertTrue(any("extra" in str(v) for v in violations))

    def test_slice_without_group_prefix_fails(self) -> None:
        doc = sample_topology()
        doc["topology"]["session"]["slices"][0]["name"] = "other::slice"
        violations = mod.validate_topology_document(doc)
        self.assertTrue(any("must start with" in str(v) for v in violations))

    def test_duplicate_slice_name_fails(self) -> None:
        doc = sample_topology()
        doc["topology"]["session"]["slices"].append(
            doc["topology"]["session"]["slices"][0]
        )
        violations = mod.validate_topology_document(doc)
        self.assertTrue(any("duplicate slice" in str(v) for v in violations))


class FrontierTests(unittest.TestCase):
    def test_valid_root_frontier_passes(self) -> None:
        doc = sample_topology()
        self.assertEqual(mod.validate_frontier(doc["topology"], doc["frontier"]), [])

    def test_unsorted_frontier_fails(self) -> None:
        doc = sample_topology()
        violations = mod.validate_frontier(doc["topology"], ["utility", "session"])
        self.assertTrue(any("sorted" in str(v) for v in violations))

    def test_duplicate_frontier_entry_fails(self) -> None:
        doc = sample_topology()
        violations = mod.validate_frontier(doc["topology"], ["session", "session"])
        self.assertTrue(any("duplicates" in str(v) for v in violations))

    def test_unknown_frontier_leaf_fails(self) -> None:
        doc = sample_topology()
        violations = mod.validate_frontier(doc["topology"], ["session", "ghost"])
        self.assertTrue(any("not a declared topology leaf" in str(v) for v in violations))

    def test_removal_is_valid_transition(self) -> None:
        doc = sample_topology()
        old = ["session", "utility"]
        new = ["utility"]
        self.assertEqual(mod.is_valid_frontier_transition(doc["topology"], old, new), [])

    def test_parent_to_all_children_is_valid_transition(self) -> None:
        doc = sample_topology()
        old = ["session", "utility"]
        new = ["session::control", "session::lifecycle", "utility"]
        self.assertEqual(mod.is_valid_frontier_transition(doc["topology"], old, new), [])

    def test_partial_split_is_rejected(self) -> None:
        doc = sample_topology()
        old = ["session", "utility"]
        new = ["session::lifecycle", "utility"]
        violations = mod.is_valid_frontier_transition(doc["topology"], old, new)
        self.assertTrue(violations, "partial split must be rejected")

    def test_arbitrary_addition_is_rejected(self) -> None:
        doc = sample_topology()
        old = ["session", "utility"]
        new = ["ghost", "session", "utility"]
        violations = mod.is_valid_frontier_transition(doc["topology"], old, new)
        self.assertTrue(violations, "arbitrary growth must be rejected")


class LiveTransitionTests(unittest.TestCase):
    def test_invalid_current_manifest_fails_closed_before_transition(self) -> None:
        violations = mod.validate_baseline_transition({}, None)
        self.assertTrue(any("current topology is invalid" in str(v) for v in violations))

    def test_initial_manifest_must_start_at_all_roots(self) -> None:
        doc = sample_topology()
        self.assertEqual(mod.validate_baseline_transition(doc, None), [])
        doc["frontier"] = ["utility"]
        violations = mod.validate_baseline_transition(doc, None)
        self.assertTrue(any("first manifest revision" in str(v) for v in violations))

    def test_live_transition_rejects_topology_mutation(self) -> None:
        previous = sample_topology()
        current = json.loads(json.dumps(previous))
        current["topology"]["utility"]["scope"].append("new.rs")
        violations = mod.validate_baseline_transition(current, previous)
        self.assertTrue(any("topology is immutable" in str(v) for v in violations))

    def test_live_transition_accepts_documented_split(self) -> None:
        previous = sample_topology()
        current = json.loads(json.dumps(previous))
        current["frontier"] = ["session::control", "session::lifecycle", "utility"]
        self.assertEqual(mod.validate_baseline_transition(current, previous), [])

    def test_live_transition_rejects_arbitrary_growth(self) -> None:
        previous = sample_topology()
        previous["frontier"] = ["utility"]
        current = json.loads(json.dumps(previous))
        current["frontier"] = ["session", "utility"]
        violations = mod.validate_baseline_transition(current, previous)
        self.assertTrue(any("arbitrary growth" in str(v) for v in violations))

    def test_pending_projection_must_equal_json_frontier(self) -> None:
        doc = sample_topology()
        self.assertEqual(
            mod.validate_pending_projection(doc, ["session", "utility"]), []
        )
        violations = mod.validate_pending_projection(doc, ["utility"])
        self.assertTrue(any("does not equal JSON frontier" in str(v) for v in violations))

    def test_pending_groups_parser_is_string_only_and_order_preserving(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "contract.rs"
            path.write_text(
                'pub(crate) const PENDING_GROUPS: &[&str] = &["session", "utility"];\n',
                encoding="utf-8",
            )
            self.assertEqual(mod.load_pending_groups(path), ["session", "utility"])


class SelectorTests(unittest.TestCase):
    def test_free_selector_passes(self) -> None:
        selector = {"kind": "free", "item": ["crate", "commands", "groups", "session", "run_save"]}
        self.assertEqual(mod.validate_selector(selector, "s"), [])

    def test_inherent_selector_passes(self) -> None:
        selector = {
            "kind": "inherent",
            "self_type": {
                "tag": "path",
                "absolute": True,
                "segments": [
                    {"name": "crate"},
                    {"name": "commands", "args": []},
                    {"name": "groups", "args": []},
                    {"name": "session", "args": []},
                    {"name": "branch", "args": []},
                    {"name": "BranchCmd", "args": []},
                ],
            },
            "method": "execute",
        }
        self.assertEqual(mod.validate_selector(selector, "s"), [])

    def test_trait_impl_selector_passes(self) -> None:
        selector = {
            "kind": "trait_impl",
            "self_type": {"tag": "path", "absolute": True, "segments": [{"name": "BranchCmd"}]},
            "trait_path": {"tag": "path", "absolute": True, "segments": [{"name": "RegisterCommand"}]},
            "method": "execute",
        }
        self.assertEqual(mod.validate_selector(selector, "s"), [])

    def test_unknown_selector_kind_fails(self) -> None:
        selector = {"kind": "static", "item": ["a", "b"]}
        violations = mod.validate_selector(selector, "s")
        self.assertTrue(any("unknown selector kind" in str(v) for v in violations))

    def test_free_selector_missing_function_fails(self) -> None:
        selector = {"kind": "free", "item": ["crate"]}
        violations = mod.validate_selector(selector, "s")
        self.assertTrue(any("module path array" in str(v) for v in violations))

    def test_inherent_selector_missing_method_fails(self) -> None:
        selector = {"kind": "inherent", "self_type": {"tag": "never"}}
        violations = mod.validate_selector(selector, "s")
        self.assertTrue(any("inherent.method" in str(v) for v in violations))

    def test_unknown_self_type_tag_fails(self) -> None:
        selector = {"kind": "inherent", "self_type": {"tag": "fn_ptr"}, "method": "execute"}
        violations = mod.validate_selector(selector, "s")
        self.assertTrue(any("unknown type node tag" in str(v) for v in violations))


class TypeAlgebraTests(unittest.TestCase):
    def test_primitive_and_never_pass(self) -> None:
        self.assertEqual(mod.validate_type_node({"tag": "primitive", "name": "u8"}, "t"), [])
        self.assertEqual(mod.validate_type_node({"tag": "never"}, "t"), [])

    def test_unknown_primitive_fails(self) -> None:
        violations = mod.validate_type_node({"tag": "primitive", "name": "u24"}, "t")
        self.assertTrue(violations)

    def test_tuple_and_slice_pass(self) -> None:
        self.assertEqual(
            mod.validate_type_node({"tag": "tuple", "elems": [{"tag": "never"}]}, "t"), []
        )
        self.assertEqual(
            mod.validate_type_node({"tag": "slice", "inner": {"tag": "primitive", "name": "u8"}}, "t"),
            [],
        )

    def test_reference_with_mut_passes(self) -> None:
        self.assertEqual(
            mod.validate_type_node(
                {"tag": "reference", "mut": True, "inner": {"tag": "primitive", "name": "str"}},
                "t",
            ),
            [],
        )

    def test_array_with_byte_len_passes(self) -> None:
        node = {
            "tag": "array",
            "inner": {"tag": "primitive", "name": "u8"},
            "len": {"tag": "int", "negative": False, "magnitude": "8", "suffix": None},
        }
        self.assertEqual(mod.validate_type_node(node, "t"), [])

    def test_generic_const_argument_normalization(self) -> None:
        # byte literal 255 with u8 suffix passes; 256 fails with field diagnostic
        ok = {"tag": "const", "atom": {"tag": "int", "negative": False, "magnitude": "255", "suffix": "u8"}}
        self.assertEqual(mod.validate_generic_arg(ok, "g"), [])
        bad = {"tag": "const", "atom": {"tag": "int", "negative": False, "magnitude": "256", "suffix": "u8"}}
        violations = mod.validate_generic_arg(bad, "g")
        self.assertTrue(any("byte value 256 exceeds" in str(v) for v in violations))


class ConstAtomTests(unittest.TestCase):
    def test_bool_passes(self) -> None:
        self.assertEqual(mod.validate_const_atom({"tag": "bool", "value": True}, "c"), [])

    def test_bool_non_bool_value_fails(self) -> None:
        violations = mod.validate_const_atom({"tag": "bool", "value": 1}, "c")
        self.assertTrue(violations)

    def test_integer_canonical_magnitude(self) -> None:
        self.assertEqual(
            mod.validate_const_atom(
                {"tag": "int", "negative": False, "magnitude": "0", "suffix": None}, "c"
            ),
            [],
        )
        self.assertEqual(
            mod.validate_const_atom(
                {"tag": "int", "negative": False, "magnitude": "42", "suffix": "u32"}, "c"
            ),
            [],
        )
        violations = mod.validate_const_atom(
            {"tag": "int", "negative": False, "magnitude": "042", "suffix": None}, "c"
        )
        self.assertTrue(any("leading zeros" in str(v) for v in violations))

    def test_negative_zero_fails(self) -> None:
        violations = mod.validate_const_atom(
            {"tag": "int", "negative": True, "magnitude": "0", "suffix": None}, "c"
        )
        self.assertTrue(any("negative zero" in str(v) for v in violations))

    def test_unknown_suffix_fails(self) -> None:
        violations = mod.validate_const_atom(
            {"tag": "int", "negative": False, "magnitude": "1", "suffix": "u33"}, "c"
        )
        self.assertTrue(any("suffix" in str(v) for v in violations))

    def test_char_scalar_passes(self) -> None:
        self.assertEqual(mod.validate_const_atom({"tag": "char", "scalar": "x"}, "c"), [])

    def test_char_multi_scalar_fails(self) -> None:
        violations = mod.validate_const_atom({"tag": "char", "scalar": "xy"}, "c")
        self.assertTrue(any("exactly one" in str(v) for v in violations))

    def test_path_const_passes(self) -> None:
        atom = {"tag": "path", "absolute": True, "segments": ["SIZE"]}
        self.assertEqual(mod.validate_const_atom(atom, "c"), [])

    def test_unknown_tag_fails(self) -> None:
        violations = mod.validate_const_atom({"tag": "float", "value": 1.0}, "c")
        self.assertTrue(any("unknown const atom tag" in str(v) for v in violations))

    def test_extra_field_fails(self) -> None:
        violations = mod.validate_const_atom(
            {"tag": "int", "negative": False, "magnitude": "1", "suffix": None, "extra": True}, "c"
        )
        self.assertTrue(any("exactly" in str(v) for v in violations))


class LiveGateTests(unittest.TestCase):
    def test_real_topology_passes_live_gate(self) -> None:
        doc = mod.load_topology()
        self.assertEqual(mod.validate_topology_document(doc), [])

    def test_topology_artifact_is_sorted_unique(self) -> None:
        doc = mod.load_topology()
        frontier = doc["frontier"]
        self.assertEqual(frontier, sorted(frontier))
        self.assertEqual(len(frontier), len(set(frontier)))
        # FEAT-018 removed the utility group from the frontier (Stage B first
        # slice); the remaining eight groups stay pending.
        self.assertEqual(
            set(frontier),
            {"memory", "plugins", "project", "skills", "session", "config", "debug", "core"},
        )


class SourceScanTests(unittest.TestCase):
    """Hermetic fixtures for the AST-resolved source scan (Task 3.5/3.6)."""

    def _write_group(self, tmpdir: Path, group: str, files: dict[str, str]) -> Path:
        """Write a synthetic group tree under a temp groups root."""
        base = tmpdir / group
        base.mkdir(parents=True, exist_ok=True)
        for name, content in files.items():
            (base / name).write_text(content, encoding="utf-8")
        return tmpdir

    def test_parse_finds_concrete_app_free_fn(self) -> None:
        source = (
            "use crate::tui::app::App;\n"
            "pub fn run_config(app: &mut App, arg: Option<&str>) -> CommandResult {\n"
            "    CommandResult::ok()\n"
            "}\n"
        )
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "config").mkdir()
            (root / "config" / "mod.rs").write_text(source, encoding="utf-8")
            items = mod.parse_rust_file(root / "config" / "mod.rs", root)
            apps = [it for it in items if it.is_concrete_app]
            self.assertEqual(len(apps), 1)
            self.assertEqual(apps[0].kind, "free")
            self.assertEqual(apps[0].qual_path, "crate::commands::groups::config::run_config")

    def test_parse_ignores_non_app_fns(self) -> None:
        source = (
            "fn helper(value: u32) -> u32 { value }\n"
            "fn run(app: &mut crate::tui::app::App, arg: Option<&str>) -> CommandResult {\n"
            "    CommandResult::ok()\n"
            "}\n"
        )
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "core").mkdir()
            (root / "core" / "mod.rs").write_text(source, encoding="utf-8")
            items = mod.parse_rust_file(root / "core" / "mod.rs", root)
            self.assertEqual(sum(1 for it in items if it.is_concrete_app), 1)

    def test_parse_finds_trait_impl_and_inherent_methods(self) -> None:
        source = (
            "pub struct BranchCmd;\n"
            "impl RegisterCommand for BranchCmd {\n"
            "    fn info() -> &'static CommandInfo { &INFO }\n"
            "    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {\n"
            "        branch(app, arg)\n"
            "    }\n"
            "}\n"
            "impl BranchCmd {\n"
            "    pub fn helper(&self) -> u32 { 1 }\n"
            "}\n"
        )
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "session").mkdir()
            (root / "session" / "mod.rs").write_text(source, encoding="utf-8")
            items = mod.parse_rust_file(root / "session" / "mod.rs", root)
            trait_exec = [it for it in items if it.kind == "trait_impl" and it.name == "execute"]
            self.assertEqual(len(trait_exec), 1)
            self.assertTrue(trait_exec[0].is_concrete_app)
            inherent = [it for it in items if it.kind == "inherent"]
            self.assertEqual(len(inherent), 1)
            self.assertEqual(inherent[0].name, "helper")
            self.assertFalse(inherent[0].is_concrete_app)

    def test_scope_file_missing_fails(self) -> None:
        violations = mod.scan_leaf_handlers(["crates/tui/src/commands/groups/core/ghost.rs"], Path("/nonexistent"))[1]
        self.assertTrue(any("missing" in str(v) for v in violations))

    def test_frontier_matches_group_source(self) -> None:
        doc = sample_topology()
        # utility scope has one concrete-App handler; session scope has one.
        doc["topology"]["utility"]["scope"] = ["utility/mod.rs"]
        doc["topology"]["session"]["scope"] = ["session/mod.rs"]
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "utility").mkdir(parents=True)
            (root / "session").mkdir(parents=True)
            (root / "utility" / "mod.rs").write_text(
                "use crate::tui::app::App;\n"
                "fn run_util(app: &mut App, arg: Option<&str>) -> CommandResult { CommandResult::ok() }\n",
                encoding="utf-8",
            )
            (root / "session" / "mod.rs").write_text(
                "use crate::tui::app::App;\n"
                "fn run_save(app: &mut App, arg: Option<&str>) -> CommandResult { CommandResult::ok() }\n",
                encoding="utf-8",
            )
            violations = mod.check_source_frontier(doc["topology"], doc["frontier"], root)
            self.assertEqual(violations, [])

    def test_cheating_removal_fails(self) -> None:
        doc = sample_topology()
        doc["topology"]["utility"]["scope"] = ["utility/mod.rs"]
        doc["topology"]["session"]["scope"] = ["session/mod.rs"]
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "utility").mkdir(parents=True)
            (root / "session").mkdir(parents=True)
            (root / "utility" / "mod.rs").write_text(
                "use crate::tui::app::App;\n"
                "fn run_util(app: &mut App, arg: Option<&str>) -> CommandResult { CommandResult::ok() }\n",
                encoding="utf-8",
            )
            (root / "session" / "mod.rs").write_text(
                "use crate::tui::app::App;\n"
                "fn run_save(app: &mut App, arg: Option<&str>) -> CommandResult { CommandResult::ok() }\n",
                encoding="utf-8",
            )
            # Cheat: remove utility from the frontier while its handler remains.
            violations = mod.check_source_frontier(doc["topology"], ["session"], root)
            self.assertTrue(any("stale-removal" in str(v) for v in violations))

    def test_stale_frontier_entry_fails(self) -> None:
        doc = sample_topology()
        doc["topology"]["utility"]["scope"] = ["utility/mod.rs"]
        doc["topology"]["session"]["scope"] = ["session/mod.rs"]
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "utility").mkdir(parents=True)
            (root / "session").mkdir(parents=True)
            # utility has NO concrete-App handler (migrated: uses a context)
            (root / "utility" / "mod.rs").write_text(
                "fn run_util(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult { CommandResult::ok() }\n",
                encoding="utf-8",
            )
            (root / "session" / "mod.rs").write_text(
                "use crate::tui::app::App;\n"
                "fn run_save(app: &mut App, arg: Option<&str>) -> CommandResult { CommandResult::ok() }\n",
                encoding="utf-8",
            )
            violations = mod.check_source_frontier(doc["topology"], ["session", "utility"], root)
            self.assertTrue(any("stale-entry" in str(v) for v in violations))

    def test_live_source_gate_passes(self) -> None:
        doc = mod.load_topology()
        violations = mod.check_source_frontier(doc["topology"], doc["frontier"])
        self.assertEqual(violations, [])


if __name__ == "__main__":
    unittest.main()
