# SWE-bench

CodeWhale's SWE-bench adapter writes the prediction file that the official
SWE-bench evaluation harness expects. It does not replace the harness; it
generates `model_patch` rows from a local task workspace.

## One Instance

Start from a workspace checked out at the SWE-bench instance base commit, with
the issue text saved locally:

```bash
codewhale swebench run \
  --instance-id django__django-12345 \
  --issue-file issue.md \
  --predictions-path all_preds.jsonl
```

`run` invokes tool-backed non-interactive mode, equivalent to
`codewhale exec --auto`, with `stream-json` output by default. When the turn
finishes, CodeWhale exports `git diff --binary --no-ext-diff` as one JSONL
prediction row:

```json
{"instance_id":"django__django-12345","model_name_or_path":"codewhale/deepseek-v4-pro","model_patch":"diff --git ..."}
```

If you already ran CodeWhale, or edited the workspace manually, export the
current diff without another model turn:

```bash
codewhale swebench export \
  --instance-id django__django-12345 \
  --predictions-path all_preds.jsonl
```

Both commands update the row for the same `instance_id` instead of appending a
duplicate row. Untracked files are marked with `git add -N` before diff export
so newly-created files appear in the patch.

## Evaluate

Install SWE-bench and Docker using the official SWE-bench setup instructions,
then pass the prediction file to the official harness:

```bash
python -m swebench.harness.run_evaluation \
  --dataset_name princeton-nlp/SWE-bench_Lite \
  --predictions_path all_preds.jsonl \
  --max_workers 1 \
  --run_id codewhale-smoke
```

On Apple Silicon, the official SWE-bench docs recommend adding
`--namespace ''` so images build locally instead of pulling Linux images.

## Batch Driver Shape

A simple batch runner should prepare each instance workspace, write the issue
body to `issue.md`, run `codewhale swebench run`, then call the harness once
on the accumulated `all_preds.jsonl`.

For reproducible runs, pin:

- CodeWhale version and commit: `codewhale --version`
- Model label: `--model-name-or-path codewhale/deepseek-v4-pro`
- Dataset and split used by the harness
- Docker platform and worker count
- The `all_preds.jsonl` file and CodeWhale stream logs

Official references:

- SWE-bench repository: https://github.com/SWE-bench/SWE-bench
- SWE-bench harness docs: https://www.swebench.com/SWE-bench/api/harness/
