# P6 – Streaks and achievements

- **Epic:** Proposed extras (optional)
- **Status:** proposed (optional)
- **Index:** [all tickets](README.md)

**Goal:** small rewards for notable calls, which give members a reason to come back and something to talk about.

**Depends on:** T11, T15. *Against the crowd* also needs P2; P3's badges move into this framework when both are built.

**Scope**

- A registry in `domain::achievements`. Each entry has a key, name, description, and a pure function over a member's settled pick history. Thresholds are in config.
- **Winner streak:** the current and best run of correct winner picks, ordered by match end time. Void picks and picks without a winner part are skipped: they neither count nor break the run. Shown on the profile, and as an optional leaderboard column.
- **Initial set:**

| Key | Name | Rule | Repeatable |
| --- | --- | --- | --- |
| `first_points` | Rookie | First settled pick with more than 0 points | no |
| `streak_5` | Hot hand | 5 correct winner picks in a row | no |
| `streak_10` | On fire | 10 correct winner picks in a row | no |
| `perfect_series` | Perfect series | Bo3/Bo5 with the exact series score and the exact score of every played map | yes, counted |
| `sniper_10` | Sniper | 10 exact map scores in total | no |
| `upset` | Against the crowd | Earned an upset bonus (P2) | yes, counted |
| `completionist` | Completionist | A winner pick on every non-void match of a followed event, checked when the event ends | yes, per event |
| `century` | Century | 100 points all time | no |

- **Storage:** `achievements_awarded` (guild\_id, user\_id, key, scope such as an event ID or none, count, first\_awarded\_at, last\_awarded\_at).
- **Evaluation** runs after every settlement (T11) for the affected members. It is a recompute from history, so a result correction can also **revoke** an achievement. A revocation happens silently, with no post.
- **Backfill:** `app achievements --backfill` evaluates all existing history once, without posting.
- **Display:** a badge grid on the profile (T15). Earned badges show their date and count; locked ones are greyed out with their rule. Each badge shows its rarity: the share of members with a settled pick who hold it. Up to 3 badges appear next to a name on the leaderboard (rarest first).
- **Announcements:** the result post (T08) adds one line per member, e.g. "Anna unlocked *Perfect series*". A guild setting turns this off.
- Icons are a small SVG set bundled with the frontend; no external assets.

**Acceptance criteria**

- [ ] Each achievement has unit tests, including void and winner-less picks inside a streak and ordering by match end time.
- [ ] A correction that breaks a streak revokes the badge without a post.
- [ ] The backfill awards the same badges as live evaluation would have, and posts nothing.
- [ ] Evaluating 50 members after a settlement takes under 2 s on a Pi 4.
