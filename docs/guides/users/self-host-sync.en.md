# Self-hosted sync: deploy the server and install the plugin

Download `NoriShell_SelfHostSync_Server_<sync-version>.zip` and `NoriShell_SelfHostSync_Plugin_<sync-version>.zip` separately from [NoriShell Releases](https://github.com/Norixor/NoriShell/releases). Both archives share a Sync version, which may differ from the NoriShell app version. The server ZIP contains macOS, Windows, and Linux binaries for x64/ARM64, plus `README.md`. **Do not extract the plugin ZIP**; import it directly into the app. Source is in [`example/self-host-sync`](../../../example/self-host-sync/readme.md).

## 1. Set up the server

1. Extract the server ZIP and select the binary for your server's OS and architecture. For example, on Linux x64, run in a terminal:

   ```sh
   ./norishell-sync-server_linux_x64
   ```

2. On first start, enter a bind port (default `8787`) and enter and confirm a **sync service password** of at least 12 bytes. Input is hidden. By default the server listens only on `127.0.0.1`; configuration and data are stored in the current user's `NoriShell/self-host-sync` configuration directory. Later launches reuse the saved configuration. The server runs in the foreground and stops when the terminal closes or you press Ctrl+C.
3. For a direct connection from another device on your LAN, explicitly run `./norishell-sync-server_linux_x64 --listen 0.0.0.0:8787`. Enter the server's actual IP and port in the plugin, such as `192.168.1.10:8787`; **do not enter `0.0.0.0`**. HTTP transmits the service password, tokens, and request metadata in plaintext. For the Internet or an untrusted network, use an HTTPS reverse proxy and keep the server bound to loopback. An HTTPS IP URL requires a trusted certificate that covers the IP. See the server ZIP's `README.md` for reverse proxy settings, `--data-dir`, background startup, and backups.

Open the server address in a browser (for example `http://127.0.0.1:8787/`) and sign in with the same service password to view read-only status: whether an encrypted exchange exists, its revision, and last write time. The server cannot display the plaintext. Back up the entire server data directory and run only one server process per directory.

### Run the server in the background

On Linux, use systemd. This example places the executable under `/opt/norishell-sync/`, uses `/var/lib/norishell-sync` for data, and runs as `norishell-sync`. `User=norishell-sync` requires that user to **already exist**. For a new installation, create the user and private data directory, then run the binary interactively as that user with **the same `--data-dir`** to set the service password; stop it with Ctrl+C:

```sh
sudo useradd --system --user-group --no-create-home --home-dir /var/lib/norishell-sync --shell /usr/sbin/nologin norishell-sync
sudo install -d -o norishell-sync -g norishell-sync -m 700 /var/lib/norishell-sync
sudo -u norishell-sync /opt/norishell-sync/norishell-sync-server_linux_x64 --data-dir /var/lib/norishell-sync
```

Do not run `useradd` again if the user already exists. For an existing installation, set `--data-dir` to **the directory it already uses**, verify the service user's access to its files, and do not replace existing data with a new empty directory. Stop any manually started process before switching to systemd.

Save the following as `/etc/systemd/system/norishell-sync.service`, adjusting paths and the user:

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

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now norishell-sync
systemctl status norishell-sync
journalctl -u norishell-sync -f
```

If `systemctl status` shows `status=217/USER`, inspect the effective `User=` with `systemctl cat norishell-sync` and check that the user exists with `getent passwd <username>`. systemd has not switched to the service user, so the server binary has not started. Correct the name or create the service user as shown above, then run `sudo systemctl reset-failed norishell-sync` and `sudo systemctl start norishell-sync`. Check data directory permissions and logs afterward.

systemd will start the server at boot and restart it after a failure. Keep `127.0.0.1:8787` when an HTTPS reverse proxy fronts the server. On macOS, use launchd; on Windows, configure Task Scheduler for startup and retry on failure. The current Windows binary does not implement the native Windows Service interface. Initialize the password first and never run two processes against the same data directory. See the [server guide](../../../example/self-host-sync/server/README.md) for more operations detail.

## 2. Import and connect the plugin

1. In NoriShell **Plugins**, import the unextracted `NoriShell_SelfHostSync_Plugin_<sync-version>.zip` and review and grant the page and `sshSync` permissions. The new plugin requires Core API 1.88; if NoriShell reports an incompatible Core API, update the app and import the matching plugin ZIP.
2. Open the new **Sync** page and select “Enter server address,” or use “Settings → Server and plugin settings” on that page. Enter a reachable `http://` or `https://` URL or `IP:port`. A bare `IP:port` uses HTTP; a bare domain uses HTTPS.
3. Select “Connect / Log in” and use the fixed username `owner` and the sync service password from step 1. NoriShell submits the password through its secure form; plugin Wasm cannot read the password or tokens. If login fails, check the address, network, server process, and password.

## 3. Run the first sync

This section describes the intended new plugin flow and is pending native acceptance. Select “Sync now.” The plugin compares the selected categories and must show a real review action when user input is needed. New devices create or unlock the local Vault; if encrypted remote data exists, enter the Vault password used by its first uploader to recover the sync key. The local Vault password may differ. The service password only grants server access. Opening the page checks sign-in status without opening approval or recovery prompts.

The self-host sync plugin selects only three business categories from Core: portable SSH hosts (including their required identities and connection rules), saved credentials, and RDP/VNC remote desktop profiles. Application preferences stay local and are excluded from the exchange. It uploads an independently encrypted exchange, **not the complete Vault file, Vault password, device auto-unlock material, Known Hosts, Agent/FIDO credentials, or external local file paths**.

Automatic synchronization is not wired in this version. Use “Sync now” explicitly on the Sync page.

When the page says review is needed without a specific error, select Review and sync. The protected Core window shows both complete outcomes and lets you choose the source for conflicting items before approving the local result; it does not ask for the same choice again. If the cloud upload has already completed, cancelling local application does not undo it. When a specific error appears, follow that message. Local ownership, sync key binding, and object restore conflicts never overwrite data automatically.

Opening the Sync page checks sign-in status and shows cloud items from the last successful read. Select Refresh remote status to fetch current data. If the network or service is temporarily unavailable, you can still browse the retained items, marked with their UTC fetch time. This is an older snapshot, not the current cloud state. A failed refresh does not discard a valid sign-in session or require another login. After an app restart, the retained non-secret list can be shown for the same server address; the local Vault must still be unlocked to fetch or sync again. No older list exists before the first successful cloud read. “Remember this operation” for sync network access covers reads and uploads to the same network destination; changing request content does not prompt again, while a changed protocol, host, port, resolved destination, or plugin package does. A direct update of an installed plugin requires fresh permission approval. With identical account settings, Core can retain a valid sign-in session and migrate the plugin's private cache. Uninstalling and importing a differently signed package does not inherit the prior package's private sign-in or cache. A changed configuration, expired session, or older Core-only cache may still require signing in or refreshing online to build the new list.

When sync deletes a host, its saved forwarding rules are deleted in the same transaction under the same deletion setting. When confirmation is required, the prompt lists the affected rules. Deleting saved rules does not stop running forwarding sessions.

Changing the service password revokes active sessions. See the [server guide](../../../example/self-host-sync/server/README.md) for operations, backups, and API details.
