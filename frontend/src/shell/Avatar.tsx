import { useState } from 'react'

/**
 * A round avatar from the image proxy. If the image fails to load, the
 * first letter of the name stands in.
 */
export function Avatar({ src, name, size }: { src: string; name: string; size: number }) {
  const [failedSrc, setFailedSrc] = useState<string | null>(null)
  const style = { width: size, height: size }

  if (failedSrc === src) {
    return (
      <span className="avatar avatar-fallback" style={style} aria-hidden="true">
        {name.trim().charAt(0).toUpperCase() || '?'}
      </span>
    )
  }
  return (
    <img
      className="avatar"
      src={src}
      alt=""
      width={size}
      height={size}
      style={style}
      onError={() => setFailedSrc(src)}
    />
  )
}
