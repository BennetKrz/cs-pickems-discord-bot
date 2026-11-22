import { Link, useLocation } from 'react-router'
import { Avatar } from '../shell/Avatar.tsx'
import { EmptyState } from '../shell/states.tsx'

/** What the leaderboard hands over when it opens a profile. */
interface ProfileState {
  name?: string
  avatarUrl?: string
}

/**
 * Another member's profile at `/members/:id`. A placeholder until the
 * profile view (T15) lands: it shows who was opened from the leaderboard
 * and leads back to it.
 */
export function MemberProfile() {
  const state = (useLocation().state ?? {}) as ProfileState
  return (
    <>
      <Link to="/leaderboard" className="back">
        ← Leaderboard
      </Link>
      {state.name && (
        <section className="profile">
          {state.avatarUrl && <Avatar src={state.avatarUrl} name={state.name} size={64} />}
          <div>
            <h1 className="profile-name">{state.name}</h1>
          </div>
        </section>
      )}
      <EmptyState title="Profiles are coming soon">
        Each member&rsquo;s picks and points will show up here.
      </EmptyState>
    </>
  )
}
