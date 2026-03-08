# AirDrop Handoff Checklist (Studio -> MacBook)

Use this after each significant run or render pass.

## Step 1: Build Handoff Package on Studio Mac

Required files:

1. `V1.mid`
2. `V1_NP_DRY.wav`
3. Final rendered V1 stems (full set for cue)

Recommended files:

1. DP session snapshot or backup copy
2. VEP project/preset export
3. Any local cue notes from Studio

## Step 2: AirDrop Transfer

1. Select package files on Studio Mac.
2. AirDrop to MacBook Air.
3. Confirm receipt on MacBook Air before deleting any temporary bundle.

## Step 3: Place Files in Archive Structure on MacBook Air

1. Create cue archive folders:
   - `dorico_exports/<CUE_NAME>/`
   - `renders/<CUE_NAME>/`
2. Move files:
   - `V1.mid`, `V1_NP_DRY.wav` -> `dorico_exports/<CUE_NAME>/`
   - rendered stems -> `renders/<CUE_NAME>/`
3. Store notes:
   - `qc_notes/<CUE_NAME>_notes.md`

## Step 4: Trace Log Update

1. Update `qc_notes/V1_CUE_QC_TEMPLATE.md` with:
   - cue name
   - render date
   - pass/fail per gate
   - issue tags (`mapping`, `gating`, `infrastructure`)
2. Add final verdict: deliverable approved `YES/NO`.
