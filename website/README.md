# Sound Multiplexer website checks

The public website is a hand-written static page. Its browser tests exercise the interactive replica and mock the GitHub release endpoint; they never contact GitHub or attempt to play audio.

From this directory, run:

```bash
npm ci
npx playwright install chromium
npm test
```

The Pages workflow runs the full suite before upload and a small smoke test against the published Pages URL afterward. It publishes only `index.html`, `styles.css`, `site.js`, and `icon.png`. Node 22 remains the website test runtime, matching the existing project CI.
