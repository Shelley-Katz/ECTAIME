# Studio Runtime Snapshot

Captured (UTC): 2026-03-09T00:32:39Z

## Live Process State

| component | pid | elapsed | executable |
|---|---:|---|---|
| Digital Performer | 1232 | 15:38:25 | /Applications/Digital Performer.app/Contents/MacOS/Digital Performer |
| Vienna Ensemble Pro (server process) | 1188 | 15:44:45 | /Applications/Vienna Ensemble Pro/Vienna Ensemble Pro Server.app/Contents/MacOS/Vienna Ensemble Pro |

## UI Verification (AppleScript / System Events)

- DP window titles include: Waghalter_MixCT_Test_01
- VEP window title: Vienna Ensemble Pro Server [Symphonova 1] 8.0.490 - Symphonovas-Mac-Studio

This confirms the active session context aligns with:
- DP project family: Waghalter MixCT test session
- VEP project family: Symphonova 1

## Project File Evidence

- DP file candidate: /Users/symphonova/Documents/DP Projects/2026 Productions/Waghalter/DP/Waghalter_MixCT_Test_01/Waghalter_MixCT_Test_01.dpdoc
  - size_bytes: 9386665
  - modified_utc: 2026-03-08T09:04:48Z
- DP autosaves directory: /Users/symphonova/Documents/DP Projects/2026 Productions/Waghalter/DP/Waghalter_MixCT_Test_01/Autosaves
  - autosave_count: 21
- VEP file candidate: /Users/symphonova/Documents/VSL/VEP Server Projects/Symphonova 1.vesp64
  - size_bytes: 3013727
  - modified_utc: 2026-03-08T09:04:54Z

## Routing/Connection Evidence

- VEP listen sockets: *:6473, *:7200
- DP->VEP established TCP connections: 12
- VEP->DP established TCP peers: 12
- DP local-port groups (contiguous triads):
  - group_1: 49213, 49214, 49215
  - group_2: 49227, 49228, 49229
  - group_3: 49242, 49243, 49244
  - group_4: 49255, 49256, 49257

Operational interpretation:
- Connection topology is consistent with 4 active DP<->VEP endpoint groups.
- This matches the expected 4 instances running context for Symphonova 1.

## Readiness Risks (Concise)

- Exact instance names/channels inside the VEP project are not exported via CLI; only process/window/socket evidence is captured.
- DP and VEP project files show last-modified at 2026-03-08 09:04Z; current in-memory state may include unsaved live edits.
- No destructive or state-mutating command was run during this snapshot.
