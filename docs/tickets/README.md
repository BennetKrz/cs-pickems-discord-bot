# Tickets

One file per ticket. Work them one at a time, in the build order below. Keep the **Status** column here and the status line in each ticket file in sync.

**Status values:** `todo` · `in progress` · `in review` · `done` · `proposed` (optional, not planned yet)

## Build order

Tickets on the same line can run in parallel.

1. T01 → T02 → T03
2. T04, T05, T16
3. T06, T07, T17
4. T08, T10
5. T09, T11
6. T12, T14
7. T13, T15, then any of P1–P7

## Epics

- **A – Foundation and data (T01–T06):** a running binary with its own database, filled from pluggable sources within a fixed API budget. Nothing Discord-specific.
- **B – Discord integration (T07–T09):** login through the Activity, a small set of bot commands, channel posts, and admin control over which events are pickable.
- **C – Picks and scoring (T10–T11):** a pick has three optional parts; each part is scored on its own. All rules are pure functions in `domain`.
- **D – Activity views (T12–T15):** vertical slices (read API plus screen), all buildable against the fixture source (T05).
- **E – Deployment and CI/CD (T16–T17):** one image plus Watchtower on the Pi, behind the bundled Caddy or an existing Traefik; a green merge to `main` reaches the Pi automatically.
- **P – Proposed extras (P1–P7):** optional, none blocks the core product. Suggested first: P4, then P1.

## Core tickets

| ID | Title | Epic | Depends on | Status |
| --- | --- | --- | --- | --- |
| [T01](T01-project-skeleton.md) | Project skeleton, configuration and local dev loop | A | – | done |
| [T02](T02-domain-model-database.md) | Canonical domain model and database | A | T01 | done |
| [T03](T03-source-adapters-reconciliation.md) | Data-source adapter framework and reconciliation | A | T02 | done |
| [T04](T04-pandascore-adapter.md) | PandaScore adapter | A | T03 | in review |
| [T05](T05-manual-source-fixtures.md) | Manual source, admin overrides and dev fixtures | A | T03 | done |
| [T06](T06-sync-scheduler-budget.md) | Sync scheduler and API request budget | A | T03, T04 | done |
| [T07](T07-discord-auth-activity-shell.md) | Discord application, authentication and Activity shell | B | T01, T02 | done |
| [T08](T08-bot-commands-announcements.md) | Bot commands, channel announcements and Activity launch | B | T07, T03 (domain events) | done |
| [T09](T09-followed-events.md) | Followed events | B | T06, T08 | done |
| [T10](T10-pick-rules-api.md) | Pick rules and pick API | C | T02, T06 (lock ticker), T07 (auth) | in review |
| [T11](T11-scoring-settlement.md) | Scoring and settlement | C | T10, T03 (domain events), T05 (void) | in review |
| [T12](T12-matches-views.md) | Matches overview and match detail | D | T07, T10, T11 | in review |
| [T13](T13-event-bracket-view.md) | Event view with bracket tree | D | T07, T12 | in review |
| [T14](T14-leaderboard.md) | Leaderboard | D | T11, T07 | in review |
| [T15](T15-player-profile.md) | Player profile and pick history | D | T11, T14 | in review |
| [T16](T16-container-compose.md) | Container image and Raspberry Pi compose stack | E | T01 (can start early; finish after T07) | done |
| [T17](T17-ci-cd-deployment.md) | CI/CD pipeline and automatic deployment | E | T16 | in review |

## Proposed extras (optional)

| ID | Title | Epic | Depends on | Status |
| --- | --- | --- | --- | --- |
| [P1](P1-lock-reminders.md) | Lock reminders | P | T08, T10 | proposed |
| [P2](P2-crowd-upset-bonus.md) | Crowd insight and upset bonus | P | T10, T11, T12 | proposed |
| [P3](P3-seasons.md) | Seasons | P | T14, T15 | proposed |
| [P4](P4-backups.md) | Backups | P | T16 | done |
| [P5](P5-admin-status-alerts.md) | Admin status screen and alerts | P | T06, T07, T08 (P4 optional) | proposed |
| [P6](P6-streaks-achievements.md) | Streaks and achievements | P | T11, T15 | proposed |
| [P7](P7-anti-copy-mode.md) | Anti-copy mode | P | T10, T12 | proposed |

## Ticket format

Every ticket has: **Goal**, **Depends on**, **Scope** (what to build), **Acceptance criteria** (testable checkboxes), and where useful **Out of scope** and **Notes**. A ticket is done when every acceptance criterion is ticked and CI is green.
