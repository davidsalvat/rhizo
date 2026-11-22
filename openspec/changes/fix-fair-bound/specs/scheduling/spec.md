## ADDED Requirements

### Requirement: Shares by weight hold for any number of domains
While cells share the CPUs by weight, the domain a CPU runs next SHALL be
chosen so that each busy domain's charged CPU time stays within one slice
(the longest charged so far) of its weight's part of the total, whatever
the number of domains and the slices' lengths. A domain's finishing
virtual time SHALL count the longest slice charged so far. In-kernel test
builds SHALL count each domain's charged time exactly, and other builds
SHALL NOT count it.

#### Scenario: Three or more domains, stretched slices
- **WHEN** three to six busy domains of any weights run slices of any length, up to 20 nominal slices each and mixed with short ones, with one CPU stepping through the queues of one to four CPUs
- **THEN** after every slice each domain's charged time is within the longest slice so far of its weight's part of the total

#### Scenario: Many cells do not buy CPU, on a busy host
- **WHEN** on one CPU a domain of 16 spinning cells and a domain of one spinning cell, with equal weights, run for at least 2 s, and until their charged time is at least 20 of the longest slice charged, while the host runs other work
- **THEN** each domain has half of their charged time within 5 %
