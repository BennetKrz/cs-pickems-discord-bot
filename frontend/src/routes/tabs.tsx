import { useOutletContext } from 'react-router'
import { Avatar } from '../shell/Avatar.tsx'
import type { ShellContext } from '../shell/Shell.tsx'
import { EmptyState } from '../shell/states.tsx'

// Placeholder for the Me tab; T15 replaces it. Matches is in ../matches/
// (T12), Events in ../events/ (T13), Leaderboard in ../leaderboard/ (T14).

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
