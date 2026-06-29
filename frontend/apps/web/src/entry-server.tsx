// @refresh reload
import { createHandler, StartServer } from "@solidjs/start/server";

// The SolidStart SSR entry. The `data-theme="dark"` on <html> selects the design-system's default
// theme; the three themes re-skin by re-pointing the SAME semantic vars (doc 10 §4) — no markup
// branches. The mobile-friendly viewport + theme-color (the perceived-performance + native-feel cue
// the design manual specifies) are set here, once.
export default createHandler(() => (
  <StartServer
    document={({ assets, children, scripts }) => (
      <html lang="en" data-theme="dark">
        <head>
          <meta charset="utf-8" />
          <meta name="viewport" content="width=device-width, initial-scale=1" />
          <meta name="color-scheme" content="dark light" />
          <title>Myelin</title>
          {assets}
        </head>
        <body>
          <div id="app">{children}</div>
          {scripts}
        </body>
      </html>
    )}
  />
));
