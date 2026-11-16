# Architecture decision records

One short file per decision that shapes the code and isn't obvious from it. Agents and humans add an ADR whenever a ticket forces a choice, such as a spike result, a library choice or a changed default.

- File name: `NNNN-short-title.md`, numbered in order, never renumbered.
- Copy [TEMPLATE.md](TEMPLATE.md).
- To change a decision, write a new ADR and mark the old one `Superseded by NNNN`. Don't edit history.

| ADR | Title | Status |
| --- | --- | --- |
| [0000](0000-initial-decisions.md) | Initial decisions from planning | accepted |
| 0001 | PandaScore free-plan coverage (written in T04) | pending |
| [0002](0002-config-and-runtime-skeleton.md) | Configuration, runtime skeleton and toolchain | accepted |
| [0003](0003-database-schema.md) | Database schema conventions | accepted |
