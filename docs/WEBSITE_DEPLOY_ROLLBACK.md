# Sound Multiplexer website deploy and rollback

The `Deploy website` workflow validates the static page before creating the GitHub Pages artifact. After deployment, `Verify deployed website` checks the returned Pages URL, the stylesheet response, and one interactive demo control at 360 px. A failed smoke check marks the workflow failed, but it does not automatically remove or replace the published site.

## Roll back a bad website change

1. In the repository's **Actions → Deploy website** history, identify the latest successful run for the desired known-good commit. Record its commit SHA and verify the run is successful.
2. Prefer a reviewed revert of the faulty commit on `main`. The regular website checks then run before Pages is redeployed, and the post-deploy smoke verifies the reverted version.
3. For urgent restoration, use **Re-run all jobs** on the successful workflow run for the known-good SHA. The run rebuilds and redeploys that exact revision through the normal Pages workflow. Do not force-push or reset `main`.
4. Confirm the post-deploy smoke job is green and revisit the `github-pages` environment URL. If the browser cache still shows the faulty assets, reload with cache disabled before treating that as a failed rollback.

The download cards retain the public Releases page as a fallback when the GitHub API is unavailable or has no matching asset. No credentials or secret values are needed for GitHub Pages deploys; the workflow uses the repository's Pages deployment identity token.
