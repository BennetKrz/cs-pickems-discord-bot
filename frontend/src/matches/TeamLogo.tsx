import { useState } from 'react'
import type { TeamRef } from '../api.ts'

/**
 * A team logo from the image proxy. Without a logo, or if it fails to load,
 * the team's acronym (or first letter) stands in; TBD shows "?".
 */
export function TeamLogo({ team, size = 28 }: { team: TeamRef | null; size?: number }) {
  const [failedSrc, setFailedSrc] = useState<string | null>(null)
  const style = { width: size, height: size }
  const src = team?.logo_url ?? null

  if (!team || !src || failedSrc === src) {
    const text = team ? (team.acronym ?? team.name).slice(0, 3).toUpperCase() : '?'
    return (
      <span className="team-logo team-logo-fallback" style={style} aria-hidden="true">
        {text}
      </span>
    )
  }
  return (
    <img
      className="team-logo"
      src={src}
      alt=""
      width={size}
      height={size}
      style={style}
      onError={() => setFailedSrc(src)}
    />
  )
}
