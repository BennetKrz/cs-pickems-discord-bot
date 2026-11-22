import type { BackTo } from '../nav.ts'

/** A link to a member's profile and the router state it hands over. */
export interface ProfileLink {
  to: string
  state: { name: string; avatarUrl: string; back: BackTo }
}

/**
 * The link to a member's profile: the Me tab for myself, else
 * `/members/:id`. An event filter carries over, so the profile shows the
 * same numbers as the list it was opened from. The name and avatar show
 * while it loads; `back` is where its back link leads.
 */
export function profileLink(
  member: { user_id: string; display_name: string; avatar_url: string },
  myUserId: string,
  back: BackTo,
  eventId: number | null = null,
): ProfileLink {
  const path = member.user_id === myUserId ? '/me' : `/members/${member.user_id}`
  return {
    to: eventId === null ? path : `${path}?event=${eventId}`,
    state: { name: member.display_name, avatarUrl: member.avatar_url, back },
  }
}
