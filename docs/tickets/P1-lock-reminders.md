# P1 – Lock reminders

- **Epic:** Proposed extras (optional)
- **Status:** proposed (optional)
- **Index:** [all tickets](README.md)

**Goal:** nobody misses a pick because they forgot.

**Depends on:** T08, T10

**Scope**

- Per-member setting in `user_settings` (guild\_id, user\_id, reminder\_mode, lead\_minutes): mode `off` (default), `dm` or `channel`; lead time 60 min by default, allowed 15–240. Set via `/remind off|dm|channel [minutes]` and a toggle on the *Me* tab.
- A job runs every minute. It finds opted-in members with **no pick** on a match whose lock falls within their lead time. A second toggle, *also remind on partial picks*, is off by default.
- **One message per member per run** lists every match that locks soon, with a *Pick now* button. One match deep-links to it (pending view, T08); several open *Matches* with the *Needs my pick* filter.
- `reminders_sent` (guild\_id, user\_id, match\_id, lock\_at) prevents duplicates. Keying on `lock_at` means a rescheduled match reminds again.
- **Channel mode:** one bundled post per run in the announce channel that mentions all due members. It counts toward the post rate limit (T08) and is deleted once the matches lock.
- **Closed DMs:** if a DM fails, fall back to channel mode for that run and log it. No retries.

**Acceptance criteria**

- [ ] Nobody gets a reminder without opting in, and nobody who has already picked gets one.
- [ ] Exactly one reminder per member, match and lock time, also across restarts.
- [ ] Reminders arrive within 2 min of the target time.
- [ ] A member with closed DMs is mentioned in the channel instead, without errors.
