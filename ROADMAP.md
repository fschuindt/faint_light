# Roadmap

## Solver
- Crowded fields (galactic plane, bulge, SMC): the only blind failures in the benchmark
- Sparse sub-degree fields: re-test with the deeper 4200 and Gaia index series
- Centre accuracy on very wide fields (11.7° TESS frames are a few arcminutes off)
- The slow tail: a few blind solves take seconds, the worst took 90 s
- Smarter index search order

## Benchmarks and tests
- Re-run the small-field benchmarks with the full index set
- All Sky Plate Solver: find out why it fails in batch, or drop it
- Charts for the regression suite, to compare runs

## Server
- Grow the `/api/v1` surface beyond `solve`
- Web UI: a history of the session's solves, and annotations (catalogue objects in the field)
- Test with other clients: Sequence Generator Pro, APT, KStars/Ekos, AstroImageJ
- Keep warm-start state across restarts
- Document which index series to use per field size

## Other
- Publish the container image alongside the binaries
- Linux tray icon (StatusNotifierItem), so minimise-to-tray is not Windows-only
- Linux distribution packages
- Optimizations
