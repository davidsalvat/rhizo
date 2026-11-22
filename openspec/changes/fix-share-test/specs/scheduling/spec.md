## ADDED Requirements

### Requirement: Shares by weight hold as charged
While cells share the CPUs by weight, each busy domain's CPU time as the
scheduler charges it SHALL stay within one slice of its weight's part of
the total, whatever the slices' lengths, including slices a busy host
stretched by stopping the virtual CPU. The time a slice lasted SHALL be
charged to the cell that held the CPU. Tests of shares SHALL measure the
charged time over enough slices that one slice is within their bound.

#### Scenario: Stretched slices
- **WHEN** two busy domains of any weights run slices of any length, up to 20 nominal slices each, with one CPU stepping through the queues of one to four CPUs
- **THEN** after every slice each domain's charged time is within the longest slice so far of its weight's part of the total

#### Scenario: Weights 300 and 100 on a busy host
- **WHEN** two spinning roots of weights 300 and 100 run on one CPU for at least 1 s, and until their charged time is at least 20 of the longest slice charged, while the host runs other work
- **THEN** the root of weight 300 has 70 to 80 % of their charged time
