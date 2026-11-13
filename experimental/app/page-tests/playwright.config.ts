// The rail page in real engines: WebKit (the macOS host's WKWebView) and
// Chromium (WebView2 on Windows, Electron on Linux). What happy-dom cannot
// tell: layout, clipping and scrolling. The page is loaded from its files with
// a stand-in host; nothing is fetched.
import { defineConfig, devices } from '@playwright/test'

export default defineConfig({
  testDir: 'browser',
  fullyParallel: true,
  reporter: 'list',
  use: { viewport: { width: 68, height: 480 } },
  projects: [
    { name: 'webkit', use: { browserName: 'webkit' } },
    { name: 'chromium', use: { ...devices['Desktop Chrome'].use ?? {}, browserName: 'chromium', viewport: { width: 68, height: 480 } } },
  ],
})
