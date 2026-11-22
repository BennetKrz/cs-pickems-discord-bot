import { Link, Navigate, useLocation, useOutletContext, useParams } from 'react-router'
import { backFrom } from '../nav.ts'
import type { ShellContext } from '../shell/Shell.tsx'
import { type ProfileFallback, ProfileView } from './ProfileView.tsx'

/** Who to show while loading, from the state `profileLink` hands over. */
function fallbackFrom(state: unknown): ProfileFallback | undefined {
  if (typeof state !== 'object' || state === null) return undefined
  const { name, avatarUrl } = state as Record<string, unknown>
  return typeof name === 'string' && typeof avatarUrl === 'string'
    ? { name, avatarUrl }
    : undefined
}

/**
 * Another member's profile at `/members/:id` (T15), opened from the
 * leaderboard or a pick list. My own ID leads to the Me tab.
 */
export function MemberProfile() {
  const { id = '' } = useParams()
  const { me } = useOutletContext<ShellContext>()
  const location = useLocation()
  if (id === me.user_id) return <Navigate to={`/me${location.search}`} replace />
  const back = backFrom(location.state) ?? { to: '/leaderboard', label: 'Leaderboard' }
  return (
    <>
      <Link to={back.to} className="back">
        ← {back.label}
      </Link>
      {/* Keyed, so another member starts with a fresh profile. */}
      <ProfileView key={id} userId={id} fallback={fallbackFrom(location.state)} />
    </>
  )
}
