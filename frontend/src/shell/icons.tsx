import type { ReactNode } from 'react'

/**
 * Tab icons, inline so the Activity loads nothing from other hosts. 20 px,
 * drawn with `currentColor`.
 */

function Icon({ children }: { children: ReactNode }) {
  return (
    <svg
      className="icon"
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  )
}

/** A calendar: the match schedule. */
export function MatchesIcon() {
  return (
    <Icon>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M3 10h18M8 2v4M16 2v4" />
    </Icon>
  )
}

export function EventsIcon() {
  return (
    <Icon>
      <path d="M8 21h8M12 17v4M7 4h10v5a5 5 0 0 1-10 0V4Z" />
      <path d="M17 6h3v1a3 3 0 0 1-3 3M7 6H4v1a3 3 0 0 0 3 3" />
    </Icon>
  )
}

export function LeaderboardIcon() {
  return (
    <Icon>
      <path d="M4 20h16M7 20v-6M12 20V8M17 20v-9" />
    </Icon>
  )
}

export function MeIcon() {
  return (
    <Icon>
      <circle cx="12" cy="8" r="4" />
      <path d="M4 21a8 8 0 0 1 16 0" />
    </Icon>
  )
}
