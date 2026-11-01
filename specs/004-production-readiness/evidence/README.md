# Release Evidence

Committed results from the two release-verification scripts:

- `scripts/compose-smoke.sh` — clean-host smoke path
- `scripts/load-ingest.sh` — sustained ingest and latency measurements

## Required header for every evidence file

```markdown
# <Smoke | Load check> — YYYY-MM-DD

- Commit: <git rev-parse --short HEAD>
- Host: <CPU model, vCPUs, RAM, disk> (reference: 4 vCPU, 8 GB RAM, local SSD)
- Docker: <docker version> / Compose <version>
- Command: <exact command line>
- Result: PASS | PASS WITH RECORDED SHORTFALL | FAIL
```

## Files

| File | Produced by |
|---|---|
| `compose-smoke-YYYY-MM-DD.md` | `scripts/compose-smoke.sh` |
| `load-check-YYYY-MM-DD.md` | `scripts/load-ingest.sh` |
| `security-review.md` | Polish task T090 |

A shortfall is only acceptable when the file records the measured value, the target, the host profile,
and an explicit acceptance note. Never overwrite an older result; add a new dated file.
