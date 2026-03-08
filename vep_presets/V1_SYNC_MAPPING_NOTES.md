# VEP / Synchron V1 Mapping Notes

Target instrument route: `DST_V1_SYNC`

## Required Controller Assignments

- VelXF: `CC1`
- Expression: `CC11`
- Attack-related controller: `CC21`

## Verification

1. Play with only `BR_V1_LONG` active.
2. Confirm CC1 and CC11 move consistently.
3. Confirm CC21 response where patch supports attack control.
4. Repeat with `BR_V1_SHORT` and `BR_V1_REP`.

## Patch Caveat

Attack behavior is articulation/patch dependent. If unsupported in selected patch, keep CC21 routing but treat as inactive for that patch.
