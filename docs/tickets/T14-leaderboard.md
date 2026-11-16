# T14 – Leaderboard

- **Epic:** D – Activity views
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** show who in the server leads.

**Depends on:** T11, T07

**Scope**

- Ranks every member with at least one settled pick: rank, avatar, name, points, settled picks, winner accuracy, exact series hits, exact map hits, and rank change since the previous match day.
- **Tie-break:** points, then exact hits (series plus map), then winner accuracy; still equal means a shared rank.
- Provisional points from live matches count and are marked as such.
- Filter: all time, or one followed event.
- The current user is highlighted, with a *jump to me* control. A row opens that member's profile (T15).
- API: `GET /api/leaderboard?event_id=`. The bot's `/leaderboard` (T08) uses the same query.

**Acceptance criteria**

- [ ] Tie-break and shared ranks are unit-tested.
- [ ] A settlement change shows up within one poll interval.
- [ ] Points per member equal the sum of their `pick_results`, checked in a test.
