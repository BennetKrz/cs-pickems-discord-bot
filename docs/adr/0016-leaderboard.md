# 0016 – Leaderboard

- **Status:** accepted
- **Date:** 2026-11-22
- **Ticket:** T14

## Context

T14 asks for a server leaderboard with these columns: rank, avatar, name, points, settled picks, winner accuracy, exact series hits, exact map hits, and the rank change since the previous match day. It also fixes the tie-break (points, then exact hits, then winner accuracy; still equal means a shared rank), says that provisional points count and are marked, and asks for an all-time or per-event filter. The bot's `/leaderboard` (T08) must use the same query. Some things were open:

- **Match days.** No ticket defines a *match day*, and nothing stores past standings. Settlement is a recompute (ADR 0011), so `pick_results` holds only the current truth.
- **Who appears**, and what *settled* means for provisional and void results.
- **How the numbers come from the breakdown**, and how the tie-break compares accuracies.
- **Profiles.** A row should open a member's profile, which only arrives with T15.

## Decision

1. **One query, a pure ranking.**
   - `store::leaderboard::load(conn, guild, event)` reads the guild's settled picks (`pick_results ⋈ picks ⋈ matches`, optionally one event), the members' names and avatars, and the guild's time zone, in one read transaction.
   - `domain::leaderboard::build` ranks them.
   - `GET /api/leaderboard?event_id=` and the bot's `/leaderboard` (top 10) both call `load`.
2. **Who appears:** members with at least one non-void result (`provisional` or `final`). **Settled picks** counts those results.
3. **Numbers** (from each result's stored breakdown):
   - points = the sum of `pick_results.points`, provisional ones included;
   - `provisional_points` / `provisional_picks` = the part from live matches (shown as "+N live");
   - winner accuracy = winner `hit` / (`hit` + `miss`); `pending` and `void` parts don't count; `null` before any is decided;
   - exact series hits = series parts with `hit`;
   - exact map hits = map `score` parts with `hit` (exact regulation score or correct overtime winner).
   - A breakdown that does not parse still counts its points, and its hits count as none. A warning is logged.
4. **Tie-break:** points, then exact hits (series + map), then winner accuracy, all descending.
   - Accuracy is compared as an exact fraction (cross-multiplied); `null` counts as 0.
   - Still equal means a shared rank in competition style (1, 1, 3).
   - Within a shared rank, rows are ordered by display name (case-insensitive), then user ID.
5. **Rank change is a recompute, no snapshots** (owner's choice).
   - A match day is a calendar day in the guild's time zone (`guild_settings.timezone`, default Europe/Berlin), dated by match start (`coalesce(started_at, scheduled_at)`).
   - The latest match day is the latest day with a non-void result.
   - The previous rank comes from the same current results, leaving out the matches that started on the latest match day.
   - The response carries `match_day`, `has_previous` and a `previous_rank` per row (`null` = new).
   - `bot::schedule`'s local-day helpers moved to `domain::localtime` for this.
6. **Filter:** the response lists the guild's followed events (latest start first), so the Activity needs no Events API (T13). Any numeric `event_id` is accepted, and an event without results gives an empty board.
7. **Activity:**
   - polls every 60 s, and every 20 s while `live`;
   - your own row is highlighted (`aria-current`), with a *Jump to me* button;
   - your own row opens `/me`; other rows open `/members/:id`, a placeholder that T15 replaces (owner's choice).

## Consequences

- No migration and no snapshot job. A correction moves current and previous ranks alike, which matches "settlement is a recompute".
- The rank shown as "previous" is not necessarily the rank someone saw yesterday, if results were corrected since.
- Each request reads every settled pick of the guild and parses its breakdown. For one server that is small; if it ever grows large, cache per guild and invalidate on settlement.
- T15 must show the same numbers for the same filter. It can reuse `domain::leaderboard::tally` for a single member.
- `store::bot::display_names` was replaced by `store::leaderboard::members`.
