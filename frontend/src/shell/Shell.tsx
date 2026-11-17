import { NavLink, Outlet } from 'react-router'
import type { Me } from '../api.ts'
import { useAuth } from '../auth-context.ts'
import { POLL_IDLE_MS, usePoll } from '../poll.ts'
import { Avatar } from './Avatar.tsx'
import { EventsIcon, LeaderboardIcon, MatchesIcon, MeIcon } from './icons.tsx'

/** What the shell hands to every tab via `useOutletContext`. */
export interface ShellContext {
  me: Me
}

const TABS = [
  { to: '/matches', label: 'Matches', Icon: MatchesIcon },
  { to: '/events', label: 'Events', Icon: EventsIcon },
  { to: '/leaderboard', label: 'Leaderboard', Icon: LeaderboardIcon },
  { to: '/me', label: 'Me', Icon: MeIcon },
] as const

/**
 * The Activity's frame: a header with the member's name and avatar, the
 * four tabs, and the current tab below. Keeps the member fresh by polling
 * `/api/me`. On narrow screens (the Discord mobile client) the tabs sit at
 * the bottom, within thumb reach.
 */
export function Shell() {
  const auth = useAuth()
  const poll = usePoll<Me>('/api/me', POLL_IDLE_MS, auth.me)
  const me = poll.data ?? auth.me

  return (
    <div className="shell">
      <div className="chrome">
        <header className="topbar">
          <span className="brand">CS2 Pick&rsquo;em</span>
          <span className="user-chip">
            <Avatar src={me.avatar_url} name={me.display_name} size={28} />
            <span className="user-name">{me.display_name}</span>
          </span>
        </header>
        <nav className="tabs" aria-label="Sections">
          {TABS.map(({ to, label, Icon }) => (
            <NavLink key={to} to={to} className="tab">
              <Icon />
              <span>{label}</span>
            </NavLink>
          ))}
        </nav>
      </div>
      {poll.error !== undefined && (
        <p className="banner" role="status">
          Connection problem. Retrying…
        </p>
      )}
      <main className="view">
        <Outlet context={{ me } satisfies ShellContext} />
      </main>
    </div>
  )
}
