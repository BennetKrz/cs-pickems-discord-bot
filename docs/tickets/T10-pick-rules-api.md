# T10 – Pick rules and pick API

- **Epic:** C – Picks and scoring
- **Status:** in review
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

- [x] Table-driven validation tests cover every format: allowed and rejected series scores, map numbers beyond the format, map scores outside the rules, empty picks.
- [x] A request 1 s after `lock_at` is rejected even if the ticker has not run yet.
- [x] Picks on TBD matches and on unfollowed events are rejected.
- [x] A winner-only pick, a map-only pick, and a full pick are all accepted.

- [x] A pick can be edited repeatedly until the lock. Each save creates a revision, and the change shows in everyone's view within one poll interval. *(API side: the ETag of `GET …/picks` changes with every edit. The view itself is T12.)*
- [x] An edit after the lock is rejected with `pick_locked`, and the UI hides the edit controls. *(API side: `GET …/picks` returns `pickable`/`locked`. Hiding the controls is T12.)*

**Notes**

- Decisions are in [ADR 0010](../adr/0010-pick-rules-and-api.md). The owner chose:
  - backend only: the UI halves of the last two criteria move to T12;
  - an empty `PUT` deletes the pick (204);
  - the revision history is at `GET /api/admin/matches/{id}/pick-revisions`;
  - a match without a start time is not pickable.
- Also decided:
  - canceled, postponed and voided matches are not pickable;
  - the error codes are `pick_locked`/`not_pickable` (409), `invalid_pick` (422) and `invalid_body` (422, a body that is not a JSON object of the pick's shape);
  - `edited` means more than one revision, not `updated_at > created_at`: the frozen fixture clock gives equal timestamps.
- Code:
  - rules: `backend/crates/domain/src/picks.rs`;
  - routes: `backend/crates/api/src/picks.rs`, plus the revisions route in `api/src/admin.rs`;
  - queries: `store::picks::{list_for_match_with_authors, list_for_user, list_revisions_for_match, count_revisions}`;
  - the shared `api::me::display_name` and `api::error::JsonBody`;
  - frontend types and `ApiClient.putPick`/`deletePick` in `frontend/src/api.ts`.
- Tests:
  - `domain::picks` (table-driven over Bo1/Bo3/Bo5: series scores, map numbers, map scores, empty picks; pickability incl. `lock_at` ± 1 s; aggregates);
  - `store/tests/repos.rs` (the new queries);
  - `api/src/picks/tests.rs` (the HTTP criteria, guild isolation, ETag after an edit, the admin history, 401s);
  - `frontend/src/api.test.ts`.
- Checked by hand on a dev build in fixture mode: create, edit, an invalid Bo1 series, two members' picks with aggregates, the admin history, and `pick_locked` after advancing the clock past `lock_at` without waiting for the ticker.
- Follow-ups:
  - **T12:** the pick form and *Everyone's picks* on top of `GET …/picks` (poll it, hide the edit controls when `pickable` is false); the soft hint when map picks and the series pick disagree; translate the error codes into messages.
  - **T11:** settle map picks by number; picks for maps that are never played are void.
  - **T09:** following an event makes its matches pickable through `followed_events`; nothing else is needed here.
  - Deleting a pick also deletes its revisions (cascade from ADR 0003), so the admin history only covers current picks.
