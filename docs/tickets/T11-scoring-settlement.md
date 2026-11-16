# T11 – Scoring and settlement

- **Epic:** C – Picks and scoring
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** points are always correct, including after corrections, and they move while a match is running.

**Depends on:** T10, T03 (domain events), T05 (void)

**Default scoring rules** (per guild, stored as JSON with a version number)

| Part | Condition | Points |
| --- | --- | --- |
| Winner | correct | 3 |
| Series score (Bo3/Bo5) | exact | 3 |
| Map winner | correct, per map | 1 |
| Map score | exact regulation score, or correct *overtime win* team, per map | 2 more |

Maximum per match: Bo1 6, Bo3 15, Bo5 21. Longer series weigh more, which suits playoffs and finals. *Default values chosen during planning; the owner may change them.*

**Scope**

- `score(pick, result, rules) -> (points, breakdown)` as a pure function in `domain`.
- **Settlement is a recompute, not a delta:** for an affected match, recompute every pick and upsert `pick_results`. The leaderboard only reads `pick_results`. That makes corrections and replays safe.
- **Triggers:** `MapUpdated` with a final map score settles that map's part as *provisional*. `MatchFinished` settles everything and marks it final. `ResultCorrected`, void and reopen re-run the match.
- **Rules snapshot:** a match uses the rules version active when it locked, so a rule change mid-event does not reshuffle history. `app rescore --all` applies current rules on purpose.
- **Void cases:** canceled match, forfeit, or admin void gives 0 points and is excluded from accuracy stats.
- **Missing map round scores** (possible on the PandaScore free plan, T04): the map winner part settles; the exact-score part stays pending. Without data after 48 h it becomes void.

**Acceptance criteria**

- [ ] Unit tests cover every rule, including overtime, unplayed maps and void cases.
- [ ] A corrected result changes points and leaderboard; running settlement twice changes nothing.
- [ ] Provisional map points appear while the match is live and become final at match end.
- [ ] Settling a match with 50 picks takes under 1 s on a Raspberry Pi 4.
