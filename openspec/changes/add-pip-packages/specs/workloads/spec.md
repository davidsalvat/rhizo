# Spec Delta

## ADDED Requirements

### Requirement: Packages from PyPI in a cell
A dynamically linked build of the pinned CPython SHALL run as a cell with
pinned, checksummed wheels from PyPI installed unmodified in its
`site-packages` (numpy, whose extensions it opens with `dlopen`, and
requests with what it needs), and print what it prints on Linux with the
same files and an empty environment. Where it is not built, the checks
SHALL report that they did not run.

#### Scenario: numpy computes
- **WHEN** the packages' CPython runs `app/numpy_demo.py` in a cell (array operations, a linear solve, a ufunc, a matrix product)
- **THEN** it prints the output recorded from Linux and exits with 0, and once the kept file pages are dropped every frame and heap allocation is back

#### Scenario: requests reaches the host
- **WHEN** the packages' CPython runs `app/requests_demo.py` in a cell with the URL of the host's fake model, through the network card
- **THEN** it prints the status, content type, detected encoding and tool call recorded from Linux and exits with 0

#### Scenario: A template that imported numpy
- **WHEN** a cell stopped after `import numpy` is suspended and resumed from the snapshot's bytes, forked, and frozen as a template that new cells are forked from
- **THEN** every copy prints the computation and exits with 0

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs both scripts natively from the top of the packed tree with an empty environment, requests against the host's fake model
- **THEN** each prints the recorded output and exits with 0

#### Scenario: A wheel that is not the pinned one
- **WHEN** a downloaded wheel's SHA-256 differs from its pin
- **THEN** the build fails, naming the URL, the expected and the actual checksum
