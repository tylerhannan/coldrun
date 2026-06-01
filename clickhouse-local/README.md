# ClickHouse local binary (not committed)

Install the latest master build (same as `curl https://clickhouse.com | sh`):

```bash
./scripts/install-clickhouse-local.sh
```

Used by `validate-parquet.sh` and `sample-parquet.sh` as the correctness reference for real `hits` Parquet.
