# MixCT Detailed Session Map (Waghalter)

## Control Bus Table

| Bus ID | Musical Role Group | Track Members (DP Main Tracks) | MIDI Channel | CC | dB Range | Default |
|---|---|---|---:|---:|---|---:|
| `STR_HI` | High Strings | `Violin I`, `Violin II` | 16 | 90 | -18..+6 | 0 dB |
| `STR_MID` | Mid Strings | `Viola` | 16 | 91 | -18..+6 | 0 dB |
| `STR_LO` | Low Strings | `Violoncello`, `Double bass` | 16 | 92 | -18..+6 | 0 dB |
| `WW_HI` | High Woodwinds | `Piccolo`, `Flute 1`, `Flute 2`, `Oboe 1`, `Oboe 2` | 16 | 93 | -18..+6 | 0 dB |
| `WW_LO` | Low Woodwinds | `English Horn`, `Clarinet (A) 1`, `Clarinet (A) 2`, `Bass Clarinet (A)`, `Bassoon 1`, `Bassoon 2`, `Contrabassoon` | 16 | 94 | -18..+6 | 0 dB |
| `HN` | Horn Choir | `Horn (F) 1`, `Horn (F) 2`, `Horn (F) 3`, `Horn (F) 4` | 16 | 95 | -18..+6 | 0 dB |
| `TPT` | Trumpet Choir | `Trumpet (A) 1`, `Trumpet (A) 2`, `Trumpet (A) 3` | 16 | 96 | -18..+6 | 0 dB |
| `BR_LO` | Low Brass | `Trombone 1`, `Trombone 2`, `Trombone 3`, `Tuba` | 16 | 97 | -18..+6 | 0 dB |
| `PERC` | Percussion | `Timpani`, `Triangle`, `Suspended Cymbal` | 16 | 98 | -18..+6 | 0 dB |
| `HARP` | Harp | `Harp` | 16 | 99 | -18..+6 | 0 dB |

## Role Policy (MVP-A)

| Role | Offset from Bus Default |
|---|---:|
| `PRIMARY` | `+0.0 dB` |
| `COUNTERPOINT` | `-3.0 dB` |
| `SECONDARY` | `-6.0 dB` |
| `ACCOMPANIMENT` | `-10.0 dB` |

## Transition Policy

1. Ramp-in smoothing: `220 ms`.
2. Ramp control-point spacing: `40 ms`.
3. Segment transitions happen at bar boundaries from directive ranges.

## Alias Vocabulary (Enabled)

1. `first violins`, `second violins`, `violins` -> `STR_HI`
2. `high ww`, `high winds` -> `WW_HI`
3. `low ww`, `low winds` -> `WW_LO`
4. `horns`, `four horns` -> `HN`
5. `trumpets` -> `TPT`
6. `low brass` -> `BR_LO`
7. `percussion`, `timpani` -> `PERC`
8. `harp` -> `HARP`

## Source of Truth

1. [mixct_session_map.waghalter.yaml](/Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml)
2. [mixct_mvp_a_cli.py](/Users/sk/ECT/orchestrator/mixct_mvp_a_cli.py)
