# 0014 – Followed events

- **Status:** accepted
- **Date:** 2026-11-22
- **Ticket:** T09

## Context

T09 lets admins choose which events are synced and pickable: `/event search | follow | unfollow | list`. Much of this already existed:

- the `followed_events` table and its store functions;
- `SyncHandle::search` (budgeted, cached 24 h per source and normalized query) and `SyncHandle::sync_event_now` (T06);
- every list (match list, digest, results, `/mypicks`) joins `followed_events`;
- the leaderboard sums `pick_results` only.

Open points:

- How does a search result, which is source data that is not stored yet, become a canonical event to follow?
- What happens with a result from a source that may not create events (only the primary source may, ADR 0004)?
- In dev builds the fixture source is not in the scheduler (ADR 0006). The dev clock drives its sync, so `/event search` could not find the fixture event.
- What does `/event follow` take, given that `/event search` already follows through its select menu?

## Decision

1. **`sync::follow`** holds the logic, so the bot and later an admin screen (P5) share it:
   - `follow_hit` ingests the chosen result through `Reconciler::ingest_event`. That creates or refreshes the event and its stages and costs no request, because the data is already there. It then resolves the canonical ID through `external_refs` and follows.
   - `follow` writes `followed_events` and `audit_log` (`follow_event`, with the guild) in one `BEGIN IMMEDIATE` transaction. After the commit, it calls `sync_event_now`, and the next tick fetches the event's matches. Following an event that is already followed changes nothing.
   - `unfollow` deletes the row and writes `audit_log` (`unfollow_event`). It touches no picks or results. The scheduler stops polling once no guild follows the event, and the lists hide its matches.
2. **A result of a non-primary source that is not linked yet** is refused with `FollowError::NeedsLink`. The reconciler has already queued it as unlinked, and the bot points to `/admin link`, then `/event follow`. That keeps ADR 0004's rule that only the primary source creates entities.
3. **Search-only sources:** `SourceEntry.search_only`. No job polls such a source and `sync_event_now` skips it, but `SyncHandle::search` asks it. In dev builds with `ENABLED_SOURCES` containing `fixture`, the app passes the `FixtureDriver`'s adapter as a search-only source. The driver keeps syncing it on the dev clock.
4. **`/event follow <event>`** autocompletes events that are already in the database and not followed in this guild (`store::events::search_known`): events that were unfollowed, linked by hand or entered through `manual`. It spends no search. New events come in through `/event search`.
5. **Components:**
   - The select menu and the unfollow buttons are answered by a `ComponentInteractionCollector` inside the command. The select waits 5 min, the confirmation 2 min, and only for the admin who ran the command.
   - Their custom IDs are `event:<interaction ID>:select|confirm|cancel`. The launch handler (T08) ignores them, and two invocations never mix.
   - On a timeout the menu or buttons are removed and nothing changes.
   - Each select option's value is the index into the results kept by the command, so no source ID has to fit into 100 characters.
6. **Unfollow confirmation** is asked only when the guild has picks on the event's matches (`store::picks::count_in_event`).
7. `/event` is a top-level command group with the same admin check as `/admin` (T07's rule, checked per interaction).

## Consequences

- AC 1 holds without a scheduler run in dev: the fixture event is already ingested by the driver, so following makes its matches appear in the list at once. The Activity polls the list every 60 s, which gives "within 1 min".
- After a restart, the bot has no record of an open select menu. The collector is gone, and a late choice shows Discord's "interaction failed". The admin searches again, which is served from the cache.
- A search result's raw payload is stored in `source_records` only when it is followed, not for every result shown.
- No HTTP endpoints for follow/unfollow yet. P5 can add them on top of `sync::follow`.
