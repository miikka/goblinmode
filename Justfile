test:
    cargo llvm-cov nextest

ci:
    cargo fmt --check
    cargo clippy
    cargo llvm-cov nextest --json | python3 scripts/check_coverage.py

cov-baseline:
    cargo llvm-cov nextest --json | python3 scripts/check_coverage.py --save-baseline
