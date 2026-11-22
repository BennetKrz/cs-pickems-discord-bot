# Architecture decision records

One short file per decision that shapes the code and isn't obvious from it. Agents and humans add an ADR whenever a ticket forces a choice, such as a spike result, a library choice or a changed default.

- File name: `NNNN-short-title.md`, numbered in order, never renumbered.
- Copy [TEMPLATE.md](TEMPLATE.md).
- To change a decision, write a new ADR and mark the old one `Superseded by NNNN`. Don't edit history.

| ADR | Title | Status |
| --- | --- | --- |
| [0000](0000-initial-decisions.md) | Initial decisions from planning | accepted |
| [0001](0001-pandascore-coverage.md) | PandaScore free-plan coverage | accepted |
| [0002](0002-config-and-runtime-skeleton.md) | Configuration, runtime skeleton and toolchain | accepted |
| [0003](0003-database-schema.md) | Database schema conventions | accepted |
| [0004](0004-source-adapters-reconciliation.md) | Source adapters and reconciliation | accepted |
| [0005](0005-manual-overrides-and-fixtures.md) | Manual overrides, admin match actions and dev fixtures | accepted |
| [0006](0006-sync-scheduler-budget.md) | Sync scheduler and request budget | accepted |
| [0007](0007-discord-auth-sessions.md) | Discord sign-in, sessions, admin check and image proxy | accepted |
| [0008](0008-container-image-and-proxy.md) | Container image, optional Caddy or existing Traefik, rate limits | accepted |
| [0009](0009-ci-cd-and-watchtower.md) | CI/CD pipeline, image tags and Watchtower | accepted |
| [0010](0010-pick-rules-and-api.md) | Pick rules and pick API | accepted |
| [0011](0011-scoring-and-settlement.md) | Scoring and settlement | accepted |
| [0012](0012-matches-views.md) | Match list, match detail and the server clock | accepted |
| [0013](0013-bot-announcements-launch.md) | Bot commands, channel posts and Activity launch | accepted |
