# T13 – Event view with bracket tree

- **Epic:** D – Activity views
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** tournament matches shown in their real structure, with elimination brackets as a tree.

**Depends on:** T07, T12

**Scope**

- *Events* tab: followed events, running first, then upcoming, then finished. The event page has one sub-tab per stage.
- **Elimination stages** (single and double elimination) as a tree: one column per round; upper bracket, lower bracket and grand final; connector lines from `feeds_a_from` and `feeds_b_from`. Empty slots read "Winner of …" or "Loser of …". Each node shows teams, score, status and my pick marker, and opens the match detail.
- **Swiss and group stages are not trees.** Show a standings table (wins–losses, map difference, round difference) and the matches per round instead. This goes beyond the original requirement, because Majors open with Swiss stages.
- Mobile: horizontal scroll and a *fit to width* toggle. Render with SVG or CSS grid; no heavy graph library.
- If a source has no bracket links, fall back to ordering by round label and slot.
- API: `GET /api/events`, `GET /api/events/{id}` (stages, matches, bracket fields).

**Acceptance criteria**

- [x] The fixture's double-elimination playoff renders with correct connectors, and a TBD slot fills when its feeder match ends. *(API: `fixture_playoff_has_the_scenario_feeds_and_fills_tbd_slots`; UI: `EventDetail.test.tsx`, `bracketLayout.test.ts`.)*
- [x] The fixture's Swiss stage shows correct standings. *(API: `fixture_swiss_standings_and_rounds`; UI: `EventDetail.test.tsx`.)*
- [x] A 16-team double-elimination bracket stays usable at 360 px width. *(Scrolls inside its box, never the page; *fit to width* scales it down. `bracketLayout.test.ts`, and checked in headless Chrome at 360 px.)*

**Notes**

- Decisions are in [ADR 0017](../adr/0017-event-view-and-brackets.md). The owner chose: winner feeds get connector lines; loser feeds are labelled only ("Loser of UB R2 #1").
- Also decided:
  - **derived positions:** PandaScore sends only side and feeds, so round and slot are derived from the feeds; stored values win;
  - **`other` stages:** a bracket if they have bracket data, else a list;
  - **Swiss rounds** are derived from each team's earlier matches in the stage;
  - **standings:** only finished matches count, voided ones included;
  - **access:** the event detail opens for any event, like the match detail, with `followed` in the answer;
  - **back link:** links from the event view carry `state.back`, so the match detail's back link returns to the stage.
- Code:
  - rules: `domain::bracket::{positions, has_bracket_data, swiss_rounds, standings}`;
  - store: `store::matches::list_event_view` (all states, no follow needed), and `live` on `guilds::followed_overview`;
  - routes: `backend/crates/api/src/events.rs`. `matches::RowContext` shares the row building with T12;
  - frontend:
    - `src/events/` (`EventsView`, `EventDetail`, `BracketView`, `StandingsView`);
    - pure helpers: `bracketLayout.ts`, `format.ts`;
    - `src/nav.ts` (back links);
    - `StatusTag`/`PickTag` exported from `MatchRow.tsx`.
- Tests:
  - `domain::bracket` (fixture shape stored and derived, 16-team double elimination from feeds, fallback, cycles, Swiss rounds, standings);
  - `store/tests/repos.rs` (`list_event_view`);
  - `api/src/events/tests.rs` (list order, followed flag, views, derived positions, 404/401/304, both fixture criteria);
  - frontend: `bracketLayout.test.ts`, `EventDetail.test.tsx`, `EventsView.test.tsx`.
- Checked by hand: a dev build in fixture mode with the fixture event followed, in headless Chrome at 360 × 780.
  - Swiss standings matched the tests. The playoff tree filled "Winner of …" slots after advancing the clock 600 min.
  - The page never scrolled sideways.
  - A synthetic 16-team double elimination built from feeds only laid out as 4 + 6 rounds plus the final, with no overlaps.
  - *Fit to width* scales that bracket to about 0.22: an overview, not readable. Scrolling is the readable mode.
- Not checked: the Discord client itself.
- Follow-ups:
  - Buchholz or other Swiss tiebreaks if wanted;
  - a stage with several groups in one table would need a group field;
  - T15 can reuse `PickTag`.
