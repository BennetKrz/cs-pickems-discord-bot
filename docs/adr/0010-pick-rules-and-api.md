# 0010 – Pick rules and pick API

- **Status:** accepted
- **Date:** 2026-11-21
- **Ticket:** T10

## Context

T10 adds the pick rules and the pick API. The ticket fixes the three pick parts, the allowed scores, the lock rule (`lock_at = scheduled_at`, or `live`, whichever comes first; the server clock decides) and the four routes. Several details were open, or touched other tickets:

- Two acceptance criteria describe UI (the change appears in everyone's view, the edit controls hide after the lock), but the pick form and the picks list belong to T12, which depends on T10.
- `PUT` replaces the whole pick, and "removing every part deletes the pick", but the ticket doesn't say what an empty `PUT` does.
- Admins should see the revision history, but no route was listed for it.
- Matches without a start time have no `lock_at`, so the time lock can't apply. They would only lock once sync reports them live, up to ~2 min late.
- The schema (T02) already enforces the structure: map number vs format, winners must be teams of the match, cascading deletes. `store::picks::save` already upserts, replaces map parts and appends a revision.

## Decision

1. **Rules are pure functions in `domain::picks`.**
   - `allowed_series(format)`: none for Bo1; `2:0 2:1 1:2 0:2` for Bo3; `3:0 3:1 3:2 2:3 1:3 0:3` for Bo5.
   - `validate(format, team_a, team_b, parts)` returns the normalized parts or a `PickError`:
     - a series score without a winner sets the winner; one that contradicts the winner is rejected;
     - map numbers must be 1…N with no duplicates, and come back sorted;
     - every map pick needs a winner from the match;
     - a map score is either `rounds` with 13 on the winner's side and 0–11 on the loser's (A:B orientation), or `overtime`, never both;
     - an empty pick is its own error (`PickError::Empty`).
   - `check_pickable(match, followed, now)`: the lock is checked first, so a late request always gets `Locked`.
     - **Locked** if `locked`, `lock_at <= now` (the same boundary as the lock ticker), or the status is `live`/`finished`.
     - **Not pickable** if the event is not followed in the guild, a team is TBD, the status is `canceled`/`postponed`, there is no `lock_at`, or the match is `voided`.
   - `aggregate(...)` computes the winner split, the distribution over every allowed series score, and the winner split per map number. Percentages are whole numbers that add up to 100 (largest-remainder method).
2. **Routes and errors.**

   | Route | Answer |
   | --- | --- |
   | `PUT /api/matches/{id}/pick` | 200 with the saved pick; 204 if the body has no parts (the pick is deleted) |
   | `DELETE /api/matches/{id}/pick` | 204, also if there was no pick |
   | `GET /api/matches/{id}/picks` | `{match_id, pickable, locked, lock_at, picks, stats}`; every member, any time, also after an unfollow |
   | `GET /api/me/picks` | `{picks}`, the latest match first |
   | `GET /api/admin/matches/{id}/pick-revisions` | every current pick's revisions, oldest first; behind the admin guard, guild from the session |

   Error codes:
   - `pick_locked` (409) and `not_pickable` (409) for writes, which must pass `check_pickable`;
   - `invalid_pick` (422) with the rule's message;
   - `invalid_body` (422) for a body that is not a JSON object of the pick's shape. `api::error::JsonBody` rejects arrays, because serde would otherwise read a struct from an array;
   - `not_found` (404) for an unknown match.
3. **Wire format.**
   - Picks use internal team IDs (`winner_team_id`) and `{a, b}` scores relative to team A/B. Unknown fields are rejected.
   - The JSON shape of a pick in responses is the same as in the request, with every part present.
4. **One transaction per write.** `store::begin_write` (`BEGIN IMMEDIATE`) wraps the steps: load the match, check the follow, check pickability at `AppState::clock` (the fake clock in fixture mode), validate, then save or delete. A request between two lock-ticker runs is rejected all the same.
5. **`edited` = more than one revision.** It is counted from `pick_revisions`, not taken from `updated_at > created_at`. Timestamps can be equal: under the frozen fixture clock, or for two saves in the same millisecond.
6. **The UI parts go to T12.** `GET …/picks` carries `pickable` and `locked` for hiding the edit controls. Its ETag (T07) changes with every edit, so the next poll shows it. The TS types and `ApiClient.putPick`/`deletePick` are in `frontend/src/api.ts`.

## Consequences

- Deleting a pick (with `DELETE` or an empty `PUT`) also deletes its revisions, by the cascade from ADR 0003. The admin history only covers current picks.
- A reopened match (T05) whose status is still `postponed` stays not pickable until the source sets it back to `scheduled`.
- T11 settles map picks by map number and voids picks for maps that were never played. The rules here never compare map picks with the series pick. The soft hint ("your map picks add up to 2:1") is left to the T12 UI.
- The pick routes have no rate limit of their own; members are authenticated and writes are cheap.
