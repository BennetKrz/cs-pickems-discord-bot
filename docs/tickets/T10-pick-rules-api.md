# T10 – Pick rules and pick API

- **Epic:** C – Picks and scoring
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** members create, change and delete picks until the match starts, and everyone in the server can see all picks.

**Depends on:** T02, T06 (lock ticker), T07 (auth)

**Scope – the three parts of a pick (at least one must be set)**

| Part | Allowed values | Notes |
| --- | --- | --- |
| Winner | team A or team B | Works for every format. |
| Series score | Bo3: 2:0, 2:1, 1:2, 0:2. Bo5: 3:0, 3:1, 3:2, 2:3, 1:3, 0:3. | Not offered for Bo1: there, 1:0 is the same as the winner. Choosing a series score sets the winner; changing the winner clears a series score that no longer fits. |
| Map pick, per map number 1…N | winner (required), plus optional score: 13:0 to 13:11 for either side, or *overtime win* | N = 1, 3 or 5. Any subset of maps may be picked. |

- **Why an overtime bucket:** CS2 maps are first to 13 rounds. At 12:12 overtime is played in sets of six rounds, so final scores like 16:14 or 19:17 occur. Exact overtime scores are too fine to predict, so all of them count as one option per team.
- Map picks are keyed by map number, not map name; names are shown once the veto is known. A pick for a map that is never played (map 3 after a 2:0) is void: no points, and it doesn't count for accuracy.
- Map picks and the series pick are not forced to agree. The UI shows a soft hint, e.g. "your map picks add up to 2:1, your series pick is 2:0".
- **Pickable** only when the match is in a followed event, both teams are known (no TBD), the status is `scheduled`, and the match is not locked.
- **Lock:** `lock_at = scheduled_at`. A match locks when the lock ticker passes `lock_at` or the status turns `live`, whichever comes first. A reschedule to a later time before the lock moves `lock_at`. Once locked, only an admin reopen (T05) unlocks it. The server clock decides; requests after the lock get error code `pick_locked`.
- **Editing:** a pick can be edited any number of times until the lock. Any part can be changed, added or removed; removing every part deletes the pick. Each save writes a row to `pick_revisions`. Everyone sees the current pick with an *edited* marker and the time of the last change; admins can see the full revision history. After the lock, picks are read-only for members until an admin reopens the match (T05).
- **API:** `PUT /api/matches/{id}/pick` (creates or edits; replaces the caller's whole pick, so a removed part is simply left out), `DELETE /api/matches/{id}/pick`, `GET /api/matches/{id}/picks` (all picks of the guild for that match with display names, plus aggregates: winner split in %, series-score distribution, per-map winner split), `GET /api/me/picks`.
- All picks are visible to every server member at all times, including before the lock.

**Acceptance criteria**

- [ ] Table-driven validation tests cover every format: allowed and rejected series scores, map numbers beyond the format, map scores outside the rules, empty picks.
- [ ] A request 1 s after `lock_at` is rejected even if the ticker has not run yet.
- [ ] Picks on TBD matches and on unfollowed events are rejected.
- [ ] A winner-only pick, a map-only pick, and a full pick are all accepted.

- [ ] A pick can be edited repeatedly until the lock. Each save creates a revision, and the change shows in everyone's view within one poll interval.
- [ ] An edit after the lock is rejected with `pick_locked`, and the UI hides the edit controls.
