// @refresh reload
import { createHandler, StartServer } from "@solidjs/start/server";

// SolidStart SSR entry with the default theme and mobile viewport metadata.
export default createHandler(
  () => (
    <StartServer
      document={({ assets, children, scripts }) => (
        <html lang="en" data-theme="dark">
          <head>
            <meta charset="utf-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1" />
            <meta name="color-scheme" content="dark light" />
            {assets}
          </head>
          <body>
            <div id="app">{children}</div>
            {scripts}
          </body>
        </html>
      )}
    />
  ),
  (event) => ({ nonce: event.locals.cspNonce }),
);
