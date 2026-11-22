import { useOutletContext } from 'react-router'
import { ProfileView } from '../members/ProfileView.tsx'
import type { ShellContext } from '../shell/Shell.tsx'

// Matches is in ../matches/ (T12), Events in ../events/ (T13), Leaderboard
// in ../leaderboard/ (T14), other members' profiles in ../members/ (T15).

/** The Me tab: my own profile and picks (T15). */
export function Me() {
  const { me } = useOutletContext<ShellContext>()
  return (
    <ProfileView
      userId={me.user_id}
      fallback={{ name: me.display_name, avatarUrl: me.avatar_url }}
    >
      <p className="profile-handle">@{me.username}</p>
      {me.is_admin && <span className="badge">Server admin</span>}
    </ProfileView>
  )
}
