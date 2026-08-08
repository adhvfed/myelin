export interface SecurityHeaderOptions {
  hsts?: boolean;
  nonce: string;
  production: boolean;
}

export function contentSecurityPolicy({ nonce, production }: SecurityHeaderOptions): string {
  const scriptSources = ["'self'", `'nonce-${nonce}'`, "'strict-dynamic'"];
  const connectSources = ["'self'"];

  // Vite's development client needs eval for source maps and a websocket for HMR. Neither exception
  // is present in the production policy.
  if (!production) {
    scriptSources.push("'unsafe-eval'");
    connectSources.push("ws:", "wss:");
  }

  return [
    "default-src 'self'",
    "base-uri 'none'",
    `script-src ${scriptSources.join(" ")}`,
    "script-src-attr 'none'",
    // The app currently expresses component layout through JSX style attributes. Script execution
    // remains nonce-protected; removing this style exception requires a separate CSS migration.
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "font-src 'self'",
    `connect-src ${connectSources.join(" ")}`,
    "media-src 'self'",
    "object-src 'none'",
    "frame-src 'none'",
    "frame-ancestors 'none'",
    "form-action 'self'",
    "manifest-src 'self'",
    "worker-src 'self' blob:",
  ].join("; ");
}

export function securityHeaders(options: SecurityHeaderOptions): Readonly<Record<string, string>> {
  const headers: Record<string, string> = {
    "Cache-Control": "no-store",
    "Content-Security-Policy": contentSecurityPolicy(options),
    "Cross-Origin-Resource-Policy": "same-origin",
    "Permissions-Policy": "camera=(), display-capture=(), geolocation=(), microphone=(), payment=(), usb=()",
    // Keep same-origin Referer available to the CSRF guard's fallback path while suppressing it on
    // every cross-origin request.
    "Referrer-Policy": "same-origin",
    "X-Content-Type-Options": "nosniff",
    "X-Frame-Options": "DENY",
    "X-Permitted-Cross-Domain-Policies": "none",
    "X-XSS-Protection": "0",
  };

  if (options.production && options.hsts) {
    // Browsers ignore HSTS received over plaintext HTTP. TLS terminators still pass this header
    // through to the HTTPS client. The explicit opt-in avoids pinning a canary/plaintext deployment
    // for a year merely because NODE_ENV was set, and does not claim sibling domains or preload.
    headers["Strict-Transport-Security"] = "max-age=31536000";
  }

  return headers;
}
