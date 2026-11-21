## ADDED Requirements

### Requirement: Sleepers found without a search
The scheduler SHALL find, take out and change a sleeping entry by its key
in time logarithmic in the number of sleepers whenever no other sleeper
put to sleep since has a key with the same low bits, without any work
when entries are taken to run or given back. Wherever its hint fails, it
SHALL find the entry by searching, so a lookup by key always gives what a
search of every place gives.

#### Scenario: Stale and shared hints
- **WHEN** entries sleep with keys sharing their low bits, some wake and are requeued, others sleep again at the same time as another
- **THEN** finding, changing and removing each by its key gives the same entry, and wake time, as a search of every queue, the waiting set and the sleepers
