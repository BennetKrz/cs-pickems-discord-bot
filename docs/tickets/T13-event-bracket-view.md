# T13 – Event view with bracket tree

- **Epic:** D – Activity views
- **Status:** todo
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

- [ ] The fixture's double-elimination playoff renders with correct connectors, and a TBD slot fills when its feeder match ends.
- [ ] The fixture's Swiss stage shows correct standings.
- [ ] A 16-team double-elimination bracket stays usable at 360 px width.
