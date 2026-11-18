import type { ReactNode } from 'react'

/** A view that is waiting for data. */
export function Loading({ label = 'Loading…' }: { label?: string }) {
  return (
    <div className="state" role="status" aria-live="polite">
      <span className="spinner" aria-hidden="true" />
      <p>{label}</p>
    </div>
  )
}

/** A view without content yet. */
export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <section className="state">
      <h2>{title}</h2>
      {children && <p>{children}</p>}
    </section>
  )
}

/** A view that failed, with an optional retry. */
export function ErrorState({
  title,
  message,
  onRetry,
}: {
  title: string
  message: string
  onRetry?: () => void
}) {
  return (
    <section className="state state-error" role="alert">
      <h2>{title}</h2>
      <p>{message}</p>
      {onRetry && (
        <button type="button" className="button" onClick={onRetry}>
          Try again
        </button>
      )}
    </section>
  )
}
