# Releasing MyJavis

One-time signing setup, then the tag-push flow. Windows is the primary target;
macOS/Android jobs in `release.yml` are `continue-on-error` until their signing
secrets exist, and `latest.json` only lists platforms with signed artifacts.

## 1. Generate the updater signing key (once)

The private key matching the `pubkey` in `tauri.conf.json` is not in this repo —
rotate it:

```bash
npm run tauri signer generate -- -w ~/.tauri/myjavis.key
```

It prompts for a password. **Store the key file and password in a password
manager** — losing the key means future releases can't update existing installs.

Then:

1. GitHub → repo **dainn-dev/assistant** → Settings → Secrets and variables → Actions:
   - `TAURI_SIGNING_PRIVATE_KEY` = **full contents** of `~/.tauri/myjavis.key`
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = the password you set
2. Paste the **public key** it printed into `src-tauri/tauri.conf.json` at
   `plugins.updater.pubkey`.
3. Commit the pubkey change.

> Existing installs built with the old pubkey can't verify the new key — they
> must install the next version manually once.

## 2. Cut a release

```bash
npm run bump 1.1.0          # syncs version in package.json, tauri.conf.json, Cargo.toml/lock
git add -A && git commit -m "release: v1.1.0"
git tag v1.1.0 && git push origin main --tags
```

Then:

1. Wait for **Build & Release** on the tag — Windows job is required, the rest
   are optional.
2. Open the created **draft** release. Check `latest.json` lists
   `windows-x86_64` with a non-empty `signature`.
3. **Publish** the release — the `releases/latest/download/latest.json`
   endpoint the updater polls ignores drafts.

## 3. Local signed build

```bash
TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/myjavis.key)" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… \
npm run tauri build
```

Expect `bundle/nsis/*.exe.sig` alongside the installer and no
"no private key" error at the end.

## 4. Verify the update path end-to-end

1. Install the previous version's NSIS build.
2. Publish the new release (step 2).
3. Launch the old install → Settings → About shows "Update v… available"
   within ~5 s → **Download & Install** → app relaunches on the new version.

Verified: _pending first real release_
