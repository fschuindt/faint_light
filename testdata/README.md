# Test data

The golden/round-trip test suites need real astrometry.net index files.
Point `FAINT_LIGHT_TEST_INDEX_DIR` at a directory containing `index-*.fits` (e.g. the ones fetched by `scripts/download_indexes.sh`) and run:

`FAINT_LIGHT_TEST_INDEX_DIR=/path/to/indexes cargo test --release`

Without the variable set, those tests skip silently.
