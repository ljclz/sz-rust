# Bench Results Directory

This directory stores benchmark results organized by timestamp.

## Naming Convention

```
benches/results/<bench_name>/<YYYY-MM-DD-HHMMSS>/
├── criterion/          # Criterion raw results
└── environment.json    # BenchEnvironment metadata
```

## Usage

```powershell
# Run benchmarks and save baseline
cargo bench --package sz-rust-core --bench core_bench -- --save-baseline phase4

# Compare with baseline
cargo bench --package sz-rust-core --bench core_bench -- --baseline phase4
```

## Constraints

- Each run preserves a separate subdirectory (by timestamp)
- Never overwrite historical results
- Single bench result ≤ 5 MB