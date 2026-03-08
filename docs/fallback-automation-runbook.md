# Fallback: DP Meter Pro Automation-Output Workflow

Use this if plugin MIDI-out is unstable in the current host path.

## Steps

1. In DP Meter Pro `Control Output`, enable automation output for:
   - `Transformed 1`
   - `RMS 1`
   - `Crest 1`
2. Write automation on playback from each ECT aux.
3. Copy resulting automation lanes to destination/control tracks.
4. Reassign continuous data:
   - Transformed -> CC1
   - RMS -> CC11
   - Crest -> CC21
5. Route resulting MIDI CC stream to `DST_V1_SYNC`.

## When to Prefer This

- Host MIDI-out path drops intermittently.
- Session needs deterministic offline repeatability.
- Deadline priority is render stability over RT elegance.
