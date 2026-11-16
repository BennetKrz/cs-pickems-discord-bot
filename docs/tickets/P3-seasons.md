# P3 – Seasons

- **Epic:** Proposed extras (optional)
- **Status:** proposed (optional)
- **Index:** [all tickets](README.md)

**Goal:** a regular fresh start for the leaderboard, while all-time history stays.

**Depends on:** T14, T15

**Scope**

- `seasons` (guild\_id, id, name, starts\_at, ends\_at, status `planned` / `active` / `closed`). Seasons of one guild never overlap. A match belongs to the season that contains its `lock_at`; matches outside any season count only for all time.
- Admin commands: `/season create <name> <start> <end>`, `/season edit`, `/season close`.
- **Closing:** automatic once `ends_at` has passed and every match of the season is final or void. After 7 days at the latest it is forced, and still-pending map parts become void.
- On close, the final standings are archived in `season_results` (rank, points, stats). The archive is the official result. Later corrections change it only through `/season recompute`.
- **Leaderboard filter** (extends T14): current season, past seasons, all time, event. The default is the current season when one is active, otherwise all time.
- **Badges** on the profile (T15): season champion (all members sharing rank 1) and event winner (rank 1 of the event leaderboard when the event ends). A post announces the top 3 when a season closes.

**Acceptance criteria**

- [ ] Match assignment is tested at the exact season boundaries.
- [ ] Overlapping seasons are rejected.
- [ ] Closing archives correct standings, posts the top 3 once, and past seasons stay viewable.
