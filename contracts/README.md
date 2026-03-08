# ECT Contracts (v1)

This folder defines canonical artifact contracts for ECT Core.

Files:

1. `input-contract.md`
2. `output-contract.md`
3. `gates.v1.yaml`
4. `metrics.schema.json`
5. `run-manifest.schema.json`
6. `config.template.yaml`
7. `profile-timeline.template.csv`
8. `note-audit.template.csv`

Rules:

1. Core outputs must conform to these contracts before a run is marked valid.
2. Contract changes must be versioned and recorded in `WORKLOG.md`.
3. Downstream tools should read contracts from this folder, not hardcoded assumptions.

