# 0017 – Event view: bracket positions, Swiss rounds and standings

- **Status:** accepted
- **Date:** 2026-11-22
- **Ticket:** T13

## Context

T13 adds the *Events* tab: followed events, and an event page with one tab per stage. Elimination stages are a tree; Swiss and group stages are standings with matches per round. The ticket leaves these points open:

- **Bracket positions.** The schema has `bracket_side`, `round_index`, `slot_index` and the two feeds.
  - The fixture sets all of them.
  - PandaScore sets only the side (from the match name) and the feeds (ADR 0001, "T13 can derive them").
  - PandaScore's playoff stages arrive as kind `other`, because it doesn't say single or double elimination.
- **Swiss rounds.** No source stores a round for Swiss matches, and neither does the fixture.
- **Standings.** The ticket names wins–losses, map difference and round difference, but not their order or which matches count.
- **Loser feeds.** In a double elimination, every upper-bracket loser drops into the lower bracket. Drawing all of those lines crosses most of the tree.
- **Access.** Should only followed events open, or any event?
- **Small screens.** A 16-team double elimination has 7 columns, far wider than 360 px.

## Decision

1. **Positions** are computed in `domain::bracket::positions`, per stage. Stored values always win; the rest is derived from the feeds.
   - **Side:** fed by any loser → `lower`. Fed by winners of both brackets (or of a final) → `final`. Fed by lower winners → `lower`. Otherwise `upper`.
   - **Round:** 1 + the highest round among winner feeders on the same side, else 0.
   - **Slot:** the rank within its column (side and round). It sorts by stored slot, else by the mean slot of the feeders on the same side (else of all feeders), then by start time, then by ID.
   - Feeds that point outside the stage are ignored. Cycles stop the recursion.
   - **The ticket's fallback:** a stage without links or positions becomes one upper column, ordered by start time, with no lines.
   - Each match carries `derived: true` when anything was derived.
2. **Swiss rounds** are computed in `domain::bracket::swiss_rounds`.
   - `round_index + 1` when it is set.
   - Otherwise, in schedule order: 1 + the most earlier matches that either known team played in the stage. Canceled matches count, because they used up a round.
   - With both teams TBD: 1 + the highest round of earlier matches that have a known team.
3. **Standings** are computed in `domain::bracket::standings`.
   - Only `finished` matches with a winner count. Voided matches count too, because a void cancels points, not the result.
   - The order is: wins (desc), losses (asc), map difference (desc), round difference (desc), then team name.
   - Every team of the stage appears, including 0–0 teams.
   - There is no Buchholz score.
4. **The stage view** (`view` in the API):
   - `single_elim` and `double_elim` stages are a `bracket`.
   - `swiss` and `groups` stages are `standings`.
   - `other` stages are a `bracket` if any match has a side or a feed inside the stage, and a `list` otherwise.
   - Matches without a stage form a trailing "Matches" list (`id: null`).
5. **Lines.** Connectors follow winner feeds within a side and into the grand final. Loser feeds get no line. Their slot reads "Loser of AUR vs FJO", or "Loser of UB R2 #1" while the feeder's teams are TBD.
6. **Layout** lives in `frontend/src/events/bracketLayout.ts`, a pure function.
   - The upper bracket is on top and the lower bracket below it. The grand final sits in the column after both, centred between the two finals.
   - A later-round node is centred on its same-side winner feeders and pushed down so that nodes never overlap.
   - Nodes are absolutely placed HTML links over one SVG layer for the lines. No graph library is used.
7. **Small screens.** The tree scrolls sideways inside its own box; the page itself never scrolls sideways. *Fit to width* scales the whole tree with a CSS transform to `container / content` (at most 1), measured with `ResizeObserver`.
8. **API.**
   - `GET /api/events` lists only followed events, running first, then upcoming, then finished and canceled.
   - `GET /api/events/{id}` opens any event, like the match detail (ADR 0012 §3). It reports `followed`; matches of an unfollowed event are not pickable.
   - Each match is the T12 `MatchRow`, with my pick and points, plus `bracket` or `round`.
9. **Polling.** The event page polls every 20 s while a match is live or locks within 24 h, else every 60 s (as ADR 0012 §8). The list polls every 60 s.
10. **Back navigation.** Links from the event view pass `state.back` (`frontend/src/nav.ts`), so the match detail's back link returns to the event's stage instead of *Matches*.

## Consequences

- PandaScore brackets render without stored rounds or slots. When a source starts sending them, they take over without code changes.
- At 360 px, a 16-team double elimination at *fit to width* scales to about 0.22. That gives an overview, but the text is too small to read; scrolling is the readable mode. This was checked in headless Chrome on a dev build.
- The standings are a plain W–L table. Real Swiss seeding (Buchholz) would be a follow-up in `domain::bracket::standings`.
- Group stages with several groups in one stage share one table. PandaScore models each group as its own tournament, so each becomes its own stage.
