# Overnight regression logs

Machine-readable logs live under `logs/overnight/` (gitignored). This folder keeps **summaries** committed for history.

| File | Description |
|------|-------------|
| [`baseline-100k.md`](baseline-100k.md) | Item 1 — 100k rows, smoke + bench Q1–10 |
| [`stress-500k.md`](stress-500k.md) | Item 2 — 500k rows stress run |

Regenerate logs:

```bash
./scripts/overnight-regression.sh 100000
./scripts/overnight-regression.sh 500000
```
