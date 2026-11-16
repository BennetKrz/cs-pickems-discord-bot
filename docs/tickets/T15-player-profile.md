# T15 – Player profile and pick history

- **Epic:** D – Activity views
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** each member sees their own score and the matches they picked, and can look at anyone else's.

**Depends on:** T11, T14

**Scope**

- The *Me* tab is the current user's profile. The same view opens for any member from the leaderboard or a pick list, because picks are public.
- Header: rank, points, number of picks, accuracy per part: winner, exact series, map winner, exact map.
- Lists: **Open** (editable, link to the match), **Live** (provisional points), **Settled** (points breakdown per match). Filter by event.
- API: `GET /api/users/{id}/profile`, `GET /api/users/{id}/picks?state=&event_id=&cursor=`.

**Acceptance criteria**

- [ ] Profile numbers equal the leaderboard numbers for the same filter.
- [ ] Void picks appear in history marked *void* and are excluded from accuracy.
- [ ] Another member's profile is reachable in two taps from the leaderboard.
