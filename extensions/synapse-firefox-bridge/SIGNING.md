# Synapse Firefox Bridge: signing and install (owner steps)

Firefox release builds (Firefox 158 on this PC) load only extensions signed by
Mozilla. Signing needs the owner's addons.mozilla.org (AMO) account, so these
steps are owner-only. The package goes to AMO as **unlisted**: Mozilla signs
it, and it is never published in the public add-on catalogue.

The registration token is **never** inside the package. The installer writes it
to Firefox managed storage (`HKCU\Software\Mozilla\ManagedStorage\synapse-bridge@synapse.local`
points to a private JSON file under `%APPDATA%\synapse\`). The installer
refuses to stage or install a package that contains `bridge-auth.json` or the
token bytes.

## 1. One-time: get AMO API credentials

1. Sign in at https://addons.mozilla.org with the owner's Mozilla account.
2. Open https://addons.mozilla.org/developers/addon/api/key/ and generate
   credentials. You get a **JWT issuer** (the API key) and a **JWT secret**.
3. Keep both private. Never commit them, and never paste them into a Synapse
   file.

## 2. One-time: install web-ext

```powershell
npm install --global web-ext
web-ext --version
```

## 3. Stage the unsigned package

```powershell
scripts\install-synapse-browser-bridge.ps1 -Browser firefox
```

The result has `status = signing_required`, the staged directory
(`%LOCALAPPDATA%\synapse\browser-extension\firefox\<build id>\source`) and the
exact `web-ext sign` command. Staging stamps the worker code before signing.
After signing, the code cannot change without a new signed version.

Before each new signed upload, raise `version` in
`extensions\synapse-firefox-bridge\manifest.json` (AMO refuses a version it has
already signed). Then stage again.

## 4. Sign (unlisted)

Run this in a new PowerShell session, so the secrets do not stay in the
environment:

```powershell
$env:WEB_EXT_API_KEY = '<JWT issuer>'
$env:WEB_EXT_API_SECRET = '<JWT secret>'
web-ext lint --source-dir "<staged dir>"
web-ext sign --channel=unlisted --source-dir "<staged dir>" --artifacts-dir "%LOCALAPPDATA%\synapse\browser-extension\firefox\signed"
Remove-Item Env:\WEB_EXT_API_KEY, Env:\WEB_EXT_API_SECRET
```

`web-ext sign` uploads the package, waits for Mozilla's automatic signing and
downloads the signed `.xpi` into the artifacts directory.

## 5. Install the signed XPI (Firefox is not touched)

Manual mode (default, no admin rights needed):

```powershell
scripts\install-synapse-gecko-bridge.ps1 -SignedXpiPath "<signed .xpi>"
```

Policy mode (force-install, needs an elevated PowerShell, because
`HKCU\Software\Policies` is read-only for normal users):

```powershell
scripts\install-synapse-gecko-bridge.ps1 -SignedXpiPath "<signed .xpi>" -InstallMode policy
```

Policy mode makes Firefox show "Your browser is being managed by your
organization".

The installer checks that the XPI is signed and has the right gecko id and no
secret. It writes the token and the managed-storage key, and reads both back.
It never starts, stops or restarts Firefox.

## 6. Restart Firefox at a time you choose

Firefox reads managed storage and policies only at startup. After the restart:

- Manual mode: open `about:addons`, click the gear, choose **Install Add-on
  From File**, pick `%LOCALAPPDATA%\synapse\browser-extension\firefox\signed\synapse-bridge@synapse.local.xpi`
  and confirm.
- Policy mode: Firefox installs the bridge by itself.

Check: `health.subsystems.firefox_bridge` shows `status=ok` and a
`bridge_host_id`.

## Test without signing (development only)

Never use the owner's daily profile for this.

- **Temporary load (any Firefox, lost on restart):** start a separate profile,
  for example `firefox.exe -P synapse-dev -no-remote`. Open
  `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on...**
  and pick `manifest.json` in the staged directory. Use this only for
  console/lint checks: Firefox never writes temporary add-ons to
  `extensions.json`, so the daemon finds no install row and refuses the
  registration origin (fail closed).
- **Unsigned permanent install (the dev loop that can register):** use only Firefox Developer Edition, Nightly
  or ESR (release builds ignore this setting). In a dedicated profile, set
  `xpinstall.signatures.required = false` in `about:config`. Then install the
  zipped staged directory (`web-ext build --source-dir "<staged dir>"`) from
  `about:addons`.

Both test paths read the token from managed storage. Write only the token
delivery (no XPI needed) before you start the test profile:

```powershell
scripts\install-synapse-gecko-bridge.ps1 -ManagedStorageOnly
```
