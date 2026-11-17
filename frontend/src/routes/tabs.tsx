import { useOutletContext } from 'react-router'
import { Avatar } from '../shell/Avatar.tsx'
import type { ShellContext } from '../shell/Shell.tsx'
import { EmptyState } from '../shell/states.tsx'

// Placeholders for the tab views. T12 (Matches), T13 (Events), T14
// (Leaderboard) and T15 (Me) replace them.

export function Matches() {
  return (
    <EmptyState title="No matches yet">
      Upcoming, live and finished matches of followed events show up here, with your picks.
    </EmptyState>
  )
}

export function Events() {
  return (
    <EmptyState title="No events yet">
      Admins choose which tournaments are open for picks.
    </EmptyState>
  )
}

export function Leaderboard() {
  return (
    <EmptyState title="No points yet">
      The leaderboard fills up once the first picked matches have finished.
    </EmptyState>
  )
}

export function Me() {
  const { me } = useOutletContext<ShellContext>()
  return (
    <>
      <section className="profile">
        <Avatar src={me.avatar_url} name={me.display_name} size={64} />
        <div>
          <h1 className="profile-name">{me.display_name}</h1>
          <p className="profile-handle">@{me.username}</p>
          {me.is_admin && <span className="badge">Server admin</span>}
        </div>
      </section>
      <EmptyState title="No picks yet">Your picks and points will show up here.</EmptyState>
    </>
  )
}
