# NoriShell self-hosted sync server

This small Go service stores one opaque encrypted exchange for one NoriShell account. It cannot read SSH profiles or Vault secrets. It listens on loopback by default; an explicit `--listen` can bind a network interface for direct HTTP access. Run one server process per data directory. Run `norishell-sync-server --version` to check the binary version; the signed-in web dashboard also displays it.

## Build and start

Go 1.23 or newer is required to build. `go test ./...` runs the server tests. `python3 build.py --output-dir dist` produces standalone binaries for macOS ARM64/x64, Windows x64/ARM64, and Linux x64/ARM64. The binaries need no Go runtime.

For a first run in a terminal, start the binary directly from any working directory:

```sh
./norishell-sync-server_linux_x64
```

Setup asks for the loopback bind port (default `8787`) and a sync password twice. Password entry is hidden. The server saves `config.json` (bind address) and `auth.json` (only an Argon2id salted password hash) under the operating system's user configuration directory, `NoriShell/self-host-sync`. Later starts reuse both settings, regardless of the working directory. On macOS this is under `~/Library/Application Support`; on Linux it follows `XDG_CONFIG_HOME` or `~/.config`; on Windows it follows the user's roaming application data directory. An existing `./data/auth.json` installation still uses that directory when launched from its old working directory. Use `--data-dir` to select an existing directory explicitly from elsewhere.

For unattended deployments, choose a private data directory, feed one password line to the existing password command, and pass the bind address on the first start:

```sh
install -d -m 700 /private/path/norishell-sync
printf '%s\n' "$PASSWORD_FROM_SECRET_MANAGER" | ./norishell-sync-server_linux_x64 password --data-dir /private/path/norishell-sync set
./norishell-sync-server_linux_x64 --data-dir /private/path/norishell-sync --listen 127.0.0.1:8787
```

The first unattended start saves the address to `config.json`; subsequent starts can omit `--listen`. `--listen` overrides the saved address for that run without changing it. Use a secret manager or a non-echoing prompt to supply `PASSWORD_FROM_SECRET_MANAGER`; clear the shell variable afterward. The password must contain 12–1024 bytes on one line. Changing it invalidates every existing access and refresh token immediately. Protect and back up the entire data directory. On Unix, files are written with mode 0600 and the directory with mode 0700. On Windows, restrict the data directory to the service account with NTFS permissions before starting the service. Without a terminal and without an initialized password, normal startup exits with setup instructions instead of starting an unauthenticated service.

The command runs in the foreground and stops with Ctrl+C. For unattended use, run the initialized binary under systemd on Linux or launchd on macOS. On Windows, use Task Scheduler to start it at boot and configure a retry on failure; this binary does not implement the Windows ServiceMain interface. Do not start a second process against the same data directory. The `User=` named in a systemd unit must exist. For a new Linux installation, create a dedicated `norishell-sync` user and private data directory, then initialize the password as that user:

```sh
sudo useradd --system --user-group --no-create-home --home-dir /var/lib/norishell-sync --shell /usr/sbin/nologin norishell-sync
sudo install -d -o norishell-sync -g norishell-sync -m 700 /var/lib/norishell-sync
sudo -u norishell-sync /opt/norishell-sync/norishell-sync-server_linux_x64 --data-dir /var/lib/norishell-sync
```

Stop the interactive server with Ctrl+C before starting systemd. Do not recreate an existing user or replace an existing data directory. A minimal unit is:

```ini
[Unit]
Description=NoriShell self-hosted sync
After=network.target

[Service]
Type=simple
User=norishell-sync
ExecStart=/opt/norishell-sync/norishell-sync-server_linux_x64 --data-dir /var/lib/norishell-sync --listen 127.0.0.1:8787
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Install the unit as `/etc/systemd/system/norishell-sync.service`. For a new installation, first run the binary interactively as the service user with the same `--data-dir` to initialize its password, then stop it. For an existing installation, stop any manually started server first. The service user must have access to the executable and the already initialized data directory; keep `--data-dir` pointed at that same directory so the service uses the existing password and exchange. Change the binary and data paths to match your installation, then run:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now norishell-sync
systemctl status norishell-sync
journalctl -u norishell-sync -f
```

The service password must be initialized before enabling the unit. With an HTTPS reverse proxy, keep the server bound to `127.0.0.1:8787`.

If the unit exits with `status=217/USER`, inspect `systemctl cat norishell-sync` and verify the configured account with `getent passwd <username>`. This status means systemd could not determine or switch to the user; the server binary has not run yet. Correct `User=` or create the missing service user, then run `sudo systemctl reset-failed norishell-sync` and `sudo systemctl start norishell-sync`. If the next failure is different, inspect `journalctl -u norishell-sync` and the executable and data-directory permissions.

For a client on another device, explicitly bind the server to a reachable interface and enter its address in the plugin:

```sh
./norishell-sync-server_linux_x64 --listen 0.0.0.0:8787
# Plugin address on another device: 192.168.1.10:8787
```

Replace `192.168.1.10` with this server's actual LAN IP. `0.0.0.0` is a bind address, not a plugin URL. The first start saves this bind address; if the server has already been initialized, `--listen` overrides its saved address for this run. Allow the port through your local firewall if needed. **HTTP sends the sync password, access tokens, and request metadata without transport encryption.** The exchange payload is encrypted by NoriShell, but that does not protect login traffic. Use HTTPS when the service is reachable over the Internet or an untrusted network.

For HTTPS, keep the default loopback bind and put a reverse proxy in front. A minimal Caddy site for a DNS name pointing to this server is:

```caddyfile
sync.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

An Nginx site using an existing trusted certificate can use:

```nginx
server {
    listen 443 ssl;
    server_name sync.example.com;
    ssl_certificate /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;
    client_max_body_size 96m;
    location / {
        proxy_set_header Host $http_host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_pass http://127.0.0.1:8787;
    }
}
```

Replace the name and certificate paths with your own values. Enter `https://sync.example.com` in the plugin. If an HTTPS plugin URL uses an IP address, its certificate must be trusted by the client and include that **IP address** in the certificate's subject alternative names; a certificate for a DNS name alone does not validate an IP URL. Configure the proxy to preserve `Authorization`, `If-Match`, `Idempotency-Key` and `X-NoriShell-*` headers, and allow bodies up to 96 MiB. Rate limit `/auth/login`; do not log request bodies, form fields, Authorization headers, or query strings. Do not expose the direct HTTP port to the public Internet or an untrusted network. Run only one server instance on the same data directory.

## Browser status page

Open the server origin in a browser, for example `http://127.0.0.1:8787/`. The `/` page uses the **same sync service password** as the plugin; no second password or account setup is needed. The page follows the browser's `Accept-Language` preference for Chinese or English, with Chinese as the fallback. On HTTP, the login page warns that the password travels in plaintext. After login it shows whether an encrypted exchange is stored, its revision, byte size, and the server's last write time. It cannot show decrypted Host settings, credentials, Vault data, or a recovery key. Use the **退出登录 / Log out** button to end the browser session. Browser sessions expire after eight hours, are lost on server restart, and become invalid immediately when the service password changes. Web and plugin password logins share a global limit of ten attempts per minute on this single-user server; excess attempts return `429` and may temporarily delay a legitimate login.

The browser form posts to `/web/login` and logout posts to `/web/logout`. Both forms contain an action-bound, signed CSRF token paired with an HttpOnly, SameSite=Strict cookie; a direct POST without first loading `/` is rejected. The logout token is also bound to the current browser session, and duplicate browser cookies are rejected. This works when a reverse proxy rewrites the upstream `Host`, as some BaoTa configurations do. The browser session and CSRF cookies are marked Secure for HTTPS requests, including requests forwarded with `X-Forwarded-Proto: https`. Configure the proxy to set `X-Forwarded-Proto` to the actual client scheme rather than forwarding an untrusted client value; preserving the original `Host` is still recommended. The browser cookie is not accepted by the plugin API, and plugin bearer tokens are not accepted by the web page. All web responses disable caching and reject query parameters. An HTTP LAN connection still exposes the service password and browser cookies in transit; use HTTPS outside a trusted local network.

If the password command reports that the update may have committed, check which password now works before retrying or restarting the service. A directory sync failure can occur after the new auth file has replaced the old one.

## NoriShell plugin configuration

Use the reachable LAN `IP:port` (interpreted as HTTP), an explicit `http://` address, or the public `https://` origin of your reverse proxy. The plugin uses these fixed paths:

| Purpose | Path |
| --- | --- |
| Login | `/auth/login` |
| Refresh | `/auth/token` |
| Revoke | `/auth/revoke` |
| Exchange GET/PUT/DELETE | `/exchange` |
| Required but disabled registration | `/auth/register` |
| Required but disabled email challenge | `/auth/email/verify` |
| Required but disabled MFA challenge | `/auth/mfa` |
| Required but unused browser authorization | `/auth/authorize` |

The plugin uses `client_id=norishell-self-host`, `scope=ssh.sync`, and username `owner`. Login and refresh return JSON `access_token`, `refresh_token`, `token_type=Bearer`, `expires_in=900`, and `scope=ssh.sync`. Access tokens live for 15 minutes; refresh tokens live for 7 days, rotate on use, and are revoked on logout. Only SHA-256 hashes of tokens are persisted. GET/PUT/DELETE `/exchange` require `Authorization: Bearer <access token>`.

The exchange body is stored byte for byte. GET on an empty exchange returns `404` with `X-NoriShell-Next-Revision`. A first PUT must send that value in `X-NoriShell-Expected-Next-Revision`; an overwrite PUT uses `If-Match` with the current strong ETag. PUT requires a UUID `Idempotency-Key` and the Core content type. Successful GET/PUT return a strong `ETag`, `X-NoriShell-Revision`, and `X-NoriShell-Updated-At-Unix-Ms`. DELETE requires the current strong `If-Match` and UUID `Idempotency-Key`, returns `204` with an empty body, and advances the next revision. Stale conditions return `412`; a reused idempotency key with different arguments returns `409`. The last 1024 write keys are retained across restarts for exact replay.
