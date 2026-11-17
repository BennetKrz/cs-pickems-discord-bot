import { createContext, useContext } from 'react'
import type { ApiClient, Me } from './api.ts'

/** What signed-in views get: the member and an API client with the session. */
export interface Auth {
  me: Me
  api: ApiClient
}

export const AuthContext = createContext<Auth | null>(null)

/** The signed-in member and API client. Only valid below `AuthGate`. */
export function useAuth(): Auth {
  const auth = useContext(AuthContext)
  if (!auth) throw new Error('useAuth() used outside a signed-in AuthGate')
  return auth
}
