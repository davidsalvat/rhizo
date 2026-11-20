# Tasks

## 1. Measure

- [x] 1.1 Diagnostic build (never merged): CPython cold start with 4 KiB pages, 2 MiB pages for the image, for the break, and both, interleaved in each benchmark boot (D3); verify two CI runs report all four modes

## 2. Loader and build

- [x] 2.1 `Image::load` maps whole 2 MiB pieces of a segment with 2 MiB pages and takes `Frames` (D1); host test of the page sizes, contents and the fallback to 4 KiB pages; verify `cargo test -p rhizo-cell`
- [ ] 2.2 CPython linked with `-z max-page-size=0x200000`, the flags in the build recipe (D2); verify its code segment starts on a 2 MiB boundary (`readelf -l`), `cargo xtask test` on both architectures, and the differential check in CI

## 3. Benchmarks

- [ ] 3.1 Verify the CI benchmark gate passes with no metric worse than on `add-image-cache`
