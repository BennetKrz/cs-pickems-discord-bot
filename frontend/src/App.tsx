import { useState } from 'react'
import { RouterProvider } from 'react-router'
import { AuthGate } from './auth.tsx'
import { createAppRouter } from './router.tsx'

export function App() {
  // Created once, outside the gate, so the current tab survives a re-sign-in.
  const [router] = useState(() => createAppRouter())
  return (
    <AuthGate>
      <RouterProvider router={router} />
    </AuthGate>
  )
}
