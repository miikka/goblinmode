test:
    cargo llvm-cov nextest

ci:
    cargo llvm-cov nextest --json | python3 scripts/check_coverage.py
