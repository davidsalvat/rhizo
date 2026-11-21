## MODIFIED Requirements

### Requirement: Continuous integration gate
Every pull request that changes code SHALL be checked automatically for
formatting, lint warnings, host tests and in-kernel tests; one that changes
only host tools SHALL be checked for formatting, lint warnings and host
tests; one that changes only documentation SHALL be checked for valid specs
and links. Any failure SHALL mark the pull request as failing.

#### Scenario: Lint warning in a pull request
- **WHEN** a pull request introduces a lint warning
- **THEN** the CI run for that pull request fails
