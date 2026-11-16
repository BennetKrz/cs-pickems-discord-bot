/** Response of `GET /healthz`. */
export interface Health {
  version: string
  git_sha: string
  db: 'ok' | 'error'
}

export async function fetchHealth(signal?: AbortSignal): Promise<Health> {
  const res = await fetch('/healthz', { signal })
  if (!res.ok && res.status !== 503) {
    throw new Error(`health check failed: HTTP ${res.status}`)
  }
  return (await res.json()) as Health
}
