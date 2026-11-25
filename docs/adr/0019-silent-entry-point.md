# 0019 – Entry Point command launches without a channel message

- **Status:** accepted
- **Date:** 2026-11-25
- **Ticket:** none (owner request after T15)

## Context

Each time a member opened the Activity from the App Launcher, Discord posted a message in the channel saying they used it. The owner wants no message on each launch.

Discord's docs, checked on 25 Nov 2026 ([Application commands](https://docs.discord.com/developers/interactions/application-commands), [User actions](https://docs.discord.com/developers/activities/development-guides/user-actions)):

- The default Entry Point command uses the handler `DISCORD_LAUNCH_ACTIVITY` (2): "Discord automatically launches your Activity for the user and posts a follow-up message into the channel where it was launched from."
- With `APP_HANDLER` (1), "the app is responsible for responding to the interaction. It can respond by launching the app's associated Activity using the `LAUNCH_ACTIVITY` (type `12`) interaction callback type."
- `handler` can be changed with Edit Global Application Command (`PATCH /applications/{app}/commands/{id}`). Fields that aren't sent stay as they are. The Developer Portal has no setting for it.
- A `LAUNCH_ACTIVITY` response creates no message. The bot's launch buttons (ADR 0013) already respond this way and post nothing.

## Decision

1. After connecting to the gateway, the bot reads the global commands. If the Entry Point command doesn't use `APP_HANDLER` yet, the bot sends `PATCH {"handler": 1}` for it. This is the only change the bot makes to global commands. It never creates, deletes or bulk-overwrites them, and it keeps the name and description.
2. The bot answers the Entry Point interaction with `LAUNCH_ACTIVITY`. It sends no message and stores no pending view.
3. poise doesn't know the command. The "unknown interaction" warning is skipped for it.
4. If the switch fails, the bot logs a warning and keeps running. Discord's handler keeps working, with its message.

This changes ADR 0013, decision 2 ("the Entry Point command stays Discord's global default").

## Consequences

- Launching from the App Launcher now needs the bot to be connected. If it isn't, Discord shows that the app didn't respond. The bot's buttons have always needed this.
- The switch is stored at Discord and stays after the bot stops. To go back, PATCH `handler` to `2` and remove the switch from the bot, or the bot switches it again on its next start.
- To announce launches later, send a follow-up message after `LAUNCH_ACTIVITY`, with the interaction token.
